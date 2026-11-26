//! 内建注册表回归：CLI、Agent、Inspector 和世界组件工厂共用同一份声明。

use xgc_base::EntityId;
use xgc_world::{builtin_registry, ComponentData, ComponentId, EntityData, World};

#[test]
fn builtin_registry_has_stable_types() {
    let registry = builtin_registry();
    let schema = registry.schema();
    let names: Vec<_> = schema.types.iter().map(|ty| ty.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "core.transform",
            "core.velocity",
            "core.name",
            "core.parent",
            "core.global_transform",
            "render.mesh",
            "render.material",
            "render.camera",
            "render.lod",
            "script.lua",
            "audio.source",
            "audio.listener",
            "audio.gain",
            "audio.mixer",
            "physics.body",
            "physics.character",
            "physics.slope",
            "animation.animator",
            "animation.skeleton",
            "animation.bone",
            "animation.pose",
            "animation.state",
            "animation.layers",
            "ui.element",
            "ui.layout",
            "ui.interaction",
            "ui.input",
            "particle.emitter",
            "plugin.rust",
            "prefab.instance",
        ]
    );
    assert!(registry.validate().is_empty());

    let second = builtin_registry().schema();
    assert_eq!(schema.types.len(), second.types.len());
    for (first, second) in schema.types.iter().zip(second.types.iter()) {
        assert_eq!(first.id, second.id);
        assert_eq!(first.name, second.name);
        assert_eq!(first.fields.len(), second.fields.len());
    }
}

#[test]
fn builtin_schema_defaults_can_construct_every_component() {
    let world = World::new();
    for schema in builtin_registry().schema().types {
        let component = ComponentId(schema.id);
        let mut data = EntityData::new(EntityId::from_raw(0));
        data.components.push(ComponentData {
            component,
            fields: schema
                .fields
                .iter()
                .map(|field| (field.name.clone(), field.default.clone()))
                .collect(),
        });
        let snapshot = world
            .snapshot_from_data(&data)
            .unwrap_or_else(|error| panic!("{} 的 schema 无法构造组件：{error}", schema.name));
        assert_eq!(snapshot.component_ids(), [component]);
        for field in &schema.fields {
            assert!(
                snapshot.get(component, &field.name).is_some(),
                "{} 缺少 schema 字段 {}",
                schema.name,
                field.name
            );
        }
    }
}
