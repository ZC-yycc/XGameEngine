//! M1 命令事务回归：计划校验 / 原子应用 / 回滚 / revision 冲突。

use std::time::Instant;

use xgc_base::EntityId;
use xgc_reflect::{ComponentId, Value};
use xgc_world::{Command, Transform, Velocity, World};

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
fn spawn_from_snapshot_and_revert() {
    let (mut world, source) = world_with_one();
    let new_id = EntityId::from_raw(1_000);
    let snapshot = world.capture_entity(source).unwrap().with_id(new_id);
    let before_hash = world.state_hash();
    let before_revision = world.revision();

    let plan = world.plan(vec![Command::Spawn { snapshot }]);
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
    let snapshot = world.capture_entity(source).unwrap();
    let before_hash = world.state_hash();
    let before_revision = world.revision();

    let plan = world.plan(vec![Command::Spawn { snapshot }]);
    assert!(!plan.is_valid());
    assert_eq!(plan.diagnostics[0].code, "EDIT.ID_IN_USE");
    assert!(world.apply(&plan).is_err());
    assert_eq!(world.state_hash(), before_hash);
    assert_eq!(world.revision(), before_revision);
}

#[test]
fn duplicate_spawn_ids_in_one_plan_are_rejected() {
    let (world, source) = world_with_one();
    let snapshot = world
        .capture_entity(source)
        .unwrap()
        .with_id(EntityId::from_raw(500));
    let plan = world.plan(vec![
        Command::Spawn {
            snapshot: snapshot.clone(),
        },
        Command::Spawn { snapshot },
    ]);
    assert!(!plan.is_valid());
    assert_eq!(plan.diagnostics[0].code, "EDIT.ID_IN_USE");
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
