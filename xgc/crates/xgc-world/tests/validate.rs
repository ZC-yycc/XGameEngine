//! 世界数据体检回归：未知组件 / 字段 / 类型 / 重复项。

use xgc_base::EntityId;
use xgc_reflect::{ComponentId, Registry, Value};
use xgc_world::{ComponentData, EntityData, Name, Transform, Velocity, World, WorldData};

fn registry() -> Registry {
    let mut registry = Registry::new();
    registry.register::<Transform>().unwrap();
    registry.register::<Velocity>().unwrap();
    registry.register::<Name>().unwrap();
    registry
}

#[test]
fn valid_world_has_no_diagnostics() {
    let mut world = World::new();
    world.spawn((
        Transform::default(),
        Velocity::default(),
        Name {
            value: "ok".to_string(),
        },
    ));
    assert!(world.validate(&registry()).is_empty());
}

#[test]
fn invalid_data_reports_structural_codes() {
    let data = WorldData {
        revision: 0,
        next_id: 10,
        entities: vec![
            EntityData {
                id: EntityId::from_raw(1),
                revision: 0,
                components: vec![
                    ComponentData {
                        component: ComponentId(0x9999),
                        fields: Vec::new(),
                    },
                    ComponentData {
                        component: ComponentId(0x0101),
                        fields: vec![
                            ("position".to_string(), Value::String("bad".to_string())),
                            ("nope".to_string(), Value::Bool(true)),
                        ],
                    },
                ],
            },
            EntityData {
                id: EntityId::from_raw(1),
                revision: 0,
                components: Vec::new(),
            },
        ],
    };

    let diagnostics = World::validate_data(&data, &registry());
    let codes: Vec<_> = diagnostics.iter().map(|diag| diag.code).collect();
    assert!(codes.contains(&"WORLD.DUP_ENTITY"));
    assert!(codes.contains(&"WORLD.UNKNOWN_COMPONENT"));
    assert!(codes.contains(&"WORLD.UNKNOWN_FIELD"));
    assert!(codes.contains(&"WORLD.TYPE_MISMATCH"));
}

#[test]
fn duplicate_component_and_field_are_reported() {
    let data = WorldData {
        revision: 0,
        next_id: 2,
        entities: vec![EntityData {
            id: EntityId::from_raw(1),
            revision: 0,
            components: vec![
                ComponentData {
                    component: ComponentId(0x0101),
                    fields: vec![
                        ("position".to_string(), Value::Vec3([0.0; 3])),
                        ("position".to_string(), Value::Vec3([1.0; 3])),
                    ],
                },
                ComponentData {
                    component: ComponentId(0x0101),
                    fields: Vec::new(),
                },
            ],
        }],
    };

    let diagnostics = World::validate_data(&data, &registry());
    let codes: Vec<_> = diagnostics.iter().map(|diag| diag.code).collect();
    assert!(codes.contains(&"WORLD.DUP_COMPONENT"));
    assert!(codes.contains(&"WORLD.DUP_FIELD"));
}
