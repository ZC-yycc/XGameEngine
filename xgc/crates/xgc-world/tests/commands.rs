//! M1 命令事务回归：计划校验 / 原子应用 / 回滚 / revision 冲突。

use std::time::Instant;

use xgc_base::EntityId;
use xgc_reflect::{ComponentId, Value};
use xgc_world::{Command, ComponentData, EntityData, Transform, Velocity, World};

fn transform() -> ComponentId {
    ComponentId(0x0101)
}

fn set_position(entity: EntityId, x: f32) -> Command {
    Command::SetField {
        entity,
        component: transform(),
        field: "position".to_string(),
        value: Value::Vec3([f64::from(x), 0.0, 0.0]),
    }
}

fn world_with_one() -> (World, EntityId) {
    let mut world = World::new();
    let key = world.spawn((Transform::default(), Velocity::default()));
    let id = world.entity(key).unwrap();
    (world, id)
}

#[test]
fn plan_apply_revert_roundtrip() {
    let (mut world, id) = world_with_one();
    let before_hash = world.state_hash();
    let before_revision = world.revision();

    let plan = world.plan(vec![set_position(id, 1.0), set_position(id, 2.0)]);
    assert!(plan.is_valid());
    assert_eq!(plan.len(), 2);

    let applied = world.apply(&plan).unwrap();
    assert_eq!(applied.revision, before_revision + 1);
    assert_eq!(world.entity_revision(id), Some(applied.revision));
    assert_eq!(
        world.get_field(id, transform(), "position"),
        Some(Value::Vec3([2.0, 0.0, 0.0]))
    );
    assert_ne!(world.state_hash(), before_hash);

    let reverted = world.revert(&applied).unwrap();
    assert_eq!(reverted, before_revision + 2);
    assert_eq!(world.state_hash(), before_hash);
    assert_eq!(world.revision(), before_revision + 2);
}

#[test]
fn invalid_plan_is_rejected_without_writing() {
    let (mut world, id) = world_with_one();
    let before_hash = world.state_hash();
    let before_revision = world.revision();

    let unknown_component = Command::SetField {
        entity: id,
        component: ComponentId(0x9999),
        field: "position".to_string(),
        value: Value::Vec3([0.0; 3]),
    };
    let plan = world.plan(vec![unknown_component]);
    assert!(!plan.is_valid());
    assert_eq!(plan.diagnostics[0].code, "EDIT.UNKNOWN_COMPONENT");
    assert!(world.apply(&plan).is_err());
    assert_eq!(world.state_hash(), before_hash);
    assert_eq!(world.revision(), before_revision);
}

#[test]
fn unknown_field_and_type_mismatch_are_reported() {
    let (world, id) = world_with_one();

    let unknown_field = Command::SetField {
        entity: id,
        component: transform(),
        field: "nope".to_string(),
        value: Value::Vec3([0.0; 3]),
    };
    let plan = world.plan(vec![unknown_field]);
    assert_eq!(plan.diagnostics[0].code, "EDIT.UNKNOWN_FIELD");

    let wrong_type = Command::SetField {
        entity: id,
        component: transform(),
        field: "position".to_string(),
        value: Value::F64(1.0),
    };
    let plan = world.plan(vec![wrong_type]);
    assert_eq!(plan.diagnostics[0].code, "EDIT.TYPE_MISMATCH");
}

#[test]
fn stale_plan_conflicts_instead_of_overwriting() {
    let (mut world, id) = world_with_one();
    let plan = world.plan(vec![set_position(id, 3.0)]);
    let applied = world.apply(&plan).unwrap();

    let err = world.apply(&plan).unwrap_err();
    assert_eq!(err[0].code, "EDIT.REVISION_CONFLICT");
    assert_eq!(applied.revision, world.revision());

    world
        .set_field(id, transform(), "scale", &Value::Vec3([2.0, 2.0, 2.0]))
        .unwrap();
    let err = world.revert(&applied).unwrap_err();
    assert_eq!(err[0].code, "EDIT.REVISION_CONFLICT");
}

#[test]
fn despawn_revert_restores_entity_and_state_hash() {
    let (mut world, id) = world_with_one();
    let before_hash = world.state_hash();
    let before_revision = world.revision();

    let plan = world.plan(vec![Command::Despawn { entity: id }]);
    assert!(plan.is_valid());
    let applied = world.apply(&plan).unwrap();
    assert!(world.key(id).is_none());
    assert_eq!(world.len(), 0);
    assert_ne!(world.state_hash(), before_hash);

    let revision = world.revert(&applied).unwrap();
    assert_eq!(revision, before_revision + 2);
    assert!(world.key(id).is_some());
    assert_eq!(world.len(), 1);
    assert_eq!(world.state_hash(), before_hash);
    assert_eq!(
        world.get_field(id, transform(), "position"),
        Some(Value::Vec3([0.0, 0.0, 0.0]))
    );
}

#[test]
fn spawn_from_data_and_revert() {
    let (mut world, source) = world_with_one();
    let new_id = EntityId::from_raw(1_000);
    let data = world.entity_data(source).unwrap().with_id(new_id);
    let before_hash = world.state_hash();
    let before_revision = world.revision();

    let plan = world.plan(vec![Command::Spawn { data }]);
    assert!(plan.is_valid());
    let applied = world.apply(&plan).unwrap();
    assert!(world.key(new_id).is_some());
    assert_eq!(world.len(), 2);
    assert_ne!(world.state_hash(), before_hash);

    world.revert(&applied).unwrap();
    assert!(world.key(new_id).is_none());
    assert_eq!(world.len(), 1);
    assert_eq!(world.state_hash(), before_hash);
    assert_eq!(world.revision(), before_revision + 2);
}

#[test]
fn spawn_id_collision_is_rejected_before_writing() {
    let (mut world, source) = world_with_one();
    let data = world.entity_data(source).unwrap();
    let before_hash = world.state_hash();
    let before_revision = world.revision();

    let plan = world.plan(vec![Command::Spawn { data }]);
    assert!(!plan.is_valid());
    assert_eq!(plan.diagnostics[0].code, "EDIT.ID_IN_USE");
    assert!(world.apply(&plan).is_err());
    assert_eq!(world.state_hash(), before_hash);
    assert_eq!(world.revision(), before_revision);
}

#[test]
fn duplicate_spawn_ids_in_one_plan_are_rejected() {
    let (world, source) = world_with_one();
    let data = world
        .entity_data(source)
        .unwrap()
        .with_id(EntityId::from_raw(500));
    let plan = world.plan(vec![
        Command::Spawn { data: data.clone() },
        Command::Spawn { data },
    ]);
    assert!(!plan.is_valid());
    assert_eq!(plan.diagnostics[0].code, "EDIT.ID_IN_USE");
}

#[test]
fn command_json_roundtrip_preserves_spawn_data() {
    let (world, source) = world_with_one();
    let data = world
        .entity_data(source)
        .unwrap()
        .with_id(EntityId::from_raw(1_000));
    let command = Command::Spawn { data };

    let json = serde_json::to_string(&command).unwrap();
    let decoded: Command = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, command);

    let mut other = World::new();
    let plan = other.plan(vec![decoded]);
    assert!(plan.is_valid());
    other.apply(&plan).unwrap();
    assert!(other.key(EntityId::from_raw(1_000)).is_some());
}

#[test]
fn set_field_after_despawn_in_same_plan_is_rejected() {
    let (world, id) = world_with_one();
    let plan = world.plan(vec![Command::Despawn { entity: id }, set_position(id, 1.0)]);
    assert!(!plan.is_valid());
    assert_eq!(plan.diagnostics[0].code, "EDIT.ENTITY_DEAD_IN_PLAN");
}

#[test]
fn one_hundred_ops_apply_and_revert() {
    let (mut world, id) = world_with_one();
    let before_hash = world.state_hash();
    let before_revision = world.revision();
    let commands = (0..100).map(|i| set_position(id, i as f32 * 0.5)).collect();

    let plan = world.plan(commands);
    assert!(plan.is_valid());
    let apply_start = Instant::now();
    let applied = world.apply(&plan).unwrap();
    let apply_time = apply_start.elapsed();
    assert_eq!(world.revision(), before_revision + 1);

    let revert_start = Instant::now();
    world.revert(&applied).unwrap();
    let revert_time = revert_start.elapsed();
    assert_eq!(world.state_hash(), before_hash);
    assert_eq!(world.revision(), before_revision + 2);

    println!("100 ops apply={apply_time:?} revert={revert_time:?}");
}

fn name_data(value: &str) -> ComponentData {
    ComponentData {
        component: ComponentId(0x0103),
        fields: vec![("value".to_string(), Value::String(value.to_string()))],
    }
}

#[test]
fn add_and_remove_component_preserve_entity_id_and_hash() {
    let (mut world, id) = world_with_one();
    let before_hash = world.state_hash();
    let before_revision = world.revision();

    let plan = world.plan(vec![Command::AddComponent {
        entity: id,
        component: name_data("hero"),
    }]);
    assert!(plan.is_valid());
    let applied = world.apply(&plan).unwrap();
    assert_eq!(applied.revision, before_revision + 1);

    let data = world.entity_data(id).unwrap();
    assert_eq!(data.id, id);
    assert!(data
        .components
        .iter()
        .any(|component| component.component == ComponentId(0x0103)));
    assert_eq!(
        world.get_field(id, ComponentId(0x0103), "value"),
        Some(Value::String("hero".to_string()))
    );
    assert_ne!(world.state_hash(), before_hash);

    let reverted = world.revert(&applied).unwrap();
    assert_eq!(reverted, before_revision + 2);
    assert_eq!(world.state_hash(), before_hash);
    assert!(!world
        .entity_data(id)
        .unwrap()
        .components
        .iter()
        .any(|component| component.component == ComponentId(0x0103)));
}

#[test]
fn remove_component_reverts_to_original_archetype() {
    let (mut world, id) = world_with_one();
    let before_hash = world.state_hash();
    let plan = world.plan(vec![Command::RemoveComponent {
        entity: id,
        component: ComponentId(0x0102),
    }]);
    assert!(plan.is_valid());
    let applied = world.apply(&plan).unwrap();

    let data = world.entity_data(id).unwrap();
    assert_eq!(data.id, id);
    assert!(!data
        .components
        .iter()
        .any(|component| component.component == ComponentId(0x0102)));

    world.revert(&applied).unwrap();
    assert_eq!(world.state_hash(), before_hash);
    assert!(world
        .entity_data(id)
        .unwrap()
        .components
        .iter()
        .any(|component| component.component == ComponentId(0x0102)));
}

#[test]
fn add_then_set_and_spawn_then_add_are_valid_in_one_plan() {
    let (mut world, id) = world_with_one();
    let before_hash = world.state_hash();
    let plan = world.plan(vec![
        Command::AddComponent {
            entity: id,
            component: name_data("old"),
        },
        Command::SetField {
            entity: id,
            component: ComponentId(0x0103),
            field: "value".to_string(),
            value: Value::String("new".to_string()),
        },
    ]);
    assert!(plan.is_valid(), "{:?}", plan.diagnostics);
    let applied = world.apply(&plan).unwrap();
    assert_eq!(
        world.get_field(id, ComponentId(0x0103), "value"),
        Some(Value::String("new".to_string()))
    );
    world.revert(&applied).unwrap();
    assert_eq!(world.state_hash(), before_hash);

    let mut world = World::new();
    let plan = world.plan(vec![
        Command::Spawn {
            data: EntityData {
                id: EntityId::from_raw(50),
                revision: 0,
                components: Vec::new(),
            },
        },
        Command::AddComponent {
            entity: EntityId::from_raw(50),
            component: name_data("spawned"),
        },
        Command::SetField {
            entity: EntityId::from_raw(50),
            component: ComponentId(0x0103),
            field: "value".to_string(),
            value: Value::String("updated".to_string()),
        },
    ]);
    assert!(plan.is_valid(), "{:?}", plan.diagnostics);
    world.apply(&plan).unwrap();
    assert_eq!(
        world.get_field(EntityId::from_raw(50), ComponentId(0x0103), "value"),
        Some(Value::String("updated".to_string()))
    );
}

#[test]
fn set_after_remove_in_same_plan_is_rejected() {
    let (world, id) = world_with_one();
    let plan = world.plan(vec![
        Command::RemoveComponent {
            entity: id,
            component: ComponentId(0x0102),
        },
        Command::SetField {
            entity: id,
            component: ComponentId(0x0102),
            field: "linear".to_string(),
            value: Value::Vec3([1.0, 0.0, 0.0]),
        },
    ]);
    assert!(!plan.is_valid());
    assert_eq!(plan.diagnostics[0].code, "EDIT.UNKNOWN_COMPONENT");
}

#[test]
fn component_structure_conflicts_are_reported() {
    let (world, id) = world_with_one();
    let existing = world.plan(vec![Command::AddComponent {
        entity: id,
        component: name_data("x"),
    }]);
    assert!(existing.is_valid());

    let plan = world.plan(vec![Command::AddComponent {
        entity: id,
        component: ComponentData {
            component: transform(),
            fields: vec![("position".to_string(), Value::Vec3([0.0; 3]))],
        },
    }]);
    assert!(!plan.is_valid());
    assert_eq!(plan.diagnostics[0].code, "EDIT.COMPONENT_EXISTS");

    let plan = world.plan(vec![Command::RemoveComponent {
        entity: id,
        component: ComponentId(0x0103),
    }]);
    assert!(!plan.is_valid());
    assert_eq!(plan.diagnostics[0].code, "EDIT.COMPONENT_MISSING");
}

#[test]
fn replace_command_swaps_and_reverts() {
    use xgc_world::{Command, Name};

    let mut world = World::new();
    let key = world.spawn((Name {
        value: "before".to_string(),
    },));
    let id = world.entity(key).unwrap();
    let mut data = world.entity_data(id).unwrap();
    for component in &mut data.components {
        for (field, value) in &mut component.fields {
            if field == "value" {
                *value = xgc_world::Value::String("after".to_string());
            }
        }
    }
    let plan = world.plan(vec![Command::Replace { data }]);
    assert!(plan.is_valid(), "{:?}", plan.diagnostics);
    let applied = world.apply(&plan).unwrap();
    let key_after = world.key(id).unwrap();
    assert_eq!(world.get::<Name>(key_after).unwrap().value, "after");
    world.revert(&applied).unwrap();
    let key_reverted = world.key(id).unwrap();
    assert_eq!(world.get::<Name>(key_reverted).unwrap().value, "before");
}
