//! 迁移回归：旧 SceneV2 RON 的实体 / 父子 / 变换映射。

use xgc_migrate::{migrate_str, MigrationError};
use xgc_world::{Component, ComponentId, GlobalTransform, Name, Parent, Transform, Value};

const SCENE: &str = r#"(
    format_version: 2,
    name: "Migrated",
    lights: [],
    ambient: (0.0, 0.0, 0.0),
    entities: [
        (
            id: (1),
            name: "Root",
            parent: None,
            layer: 0,
            transform: (
                position: (10.0, 0.0, 0.0),
                rotation: (0.0, 0.0, 0.0, 1.0),
                scale: (1.0, 1.0, 1.0),
            ),
            transform_identity: (id: 1),
            components: (
                next_component_id: (next_id: 2),
                entries: [],
            ),
        ),
        (
            id: (2),
            name: "Child",
            parent: Some((1)),
            layer: 0,
            transform: (
                position: (1.0, 2.0, 3.0),
                rotation: (0.0, 0.0, 0.0, 1.0),
                scale: (2.0, 2.0, 2.0),
            ),
            transform_identity: (id: 2),
            components: (
                next_component_id: (next_id: 1),
                entries: [],
            ),
        ),
    ],
)"#;

#[test]
fn migrates_identity_transform_and_parent() {
    let (world, report) = migrate_str(SCENE).unwrap();
    assert_eq!(report.entities, 2);
    assert_eq!(report.with_old_components, 2);
    assert_eq!(report.missing_parents, 0);
    assert_eq!(world.next_id, 3);
    assert_eq!(world.entities.len(), 2);

    let root = &world.entities[0];
    assert_eq!(root.id.raw(), 1);
    assert!(root
        .components
        .iter()
        .any(|component| component.component == Transform::ID));
    assert!(root
        .components
        .iter()
        .any(|component| component.component == Name::ID));
    assert!(root
        .components
        .iter()
        .any(|component| component.component == GlobalTransform::ID));

    let child = &world.entities[1];
    let parent = child
        .components
        .iter()
        .find(|component| component.component == Parent::ID)
        .unwrap();
    assert_eq!(parent.fields[0].1, Value::Entity(1));
    let transform = child
        .components
        .iter()
        .find(|component| component.component == Transform::ID)
        .unwrap();
    assert_eq!(transform.fields[0].1, Value::Vec3([1.0, 2.0, 3.0]));
    assert_eq!(transform.fields[2].1, Value::Vec3([2.0, 2.0, 2.0]));

    assert_eq!(
        xgc_migrate::component_id_name(ComponentId(0x0101)),
        "core.transform"
    );
}

#[test]
fn unsupported_version_and_duplicate_ids_are_rejected() {
    let wrong = SCENE.replace("format_version: 2", "format_version: 1");
    assert!(matches!(
        migrate_str(&wrong),
        Err(MigrationError::UnsupportedVersion(1))
    ));

    let duplicate = SCENE.replace("id: (2)", "id: (1)");
    assert!(matches!(
        migrate_str(&duplicate),
        Err(MigrationError::DuplicateId(1))
    ));
}

#[test]
fn missing_parent_is_counted_not_fatal() {
    let scene = SCENE.replace("Some((1))", "Some((99))");
    let (_, report) = migrate_str(&scene).unwrap();
    assert_eq!(report.missing_parents, 1);
}
