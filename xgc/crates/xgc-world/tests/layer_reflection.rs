//! 混合层通过世界事务编辑，回滚和恢复要保留层的配置及运行状态。

use xgc_reflect::{FieldDescriptor, FieldType};
use xgc_world::{AnimationLayers, Command, Component, Value, World};

#[test]
fn edits_and_restores_nested_animation_layer_fields() {
    let mut world = World::new();
    let key = world.spawn((AnimationLayers::default(),));
    let entity = world.entity(key).unwrap();
    let [field]: [FieldDescriptor; 1] = AnimationLayers::fields().try_into().unwrap();
    let FieldType::List(layer) = field.ty else {
        panic!("混合层应是结构化列表");
    };
    let set = |field: &str, value| Command::SetField {
        entity,
        component: AnimationLayers::ID,
        field: field.into(),
        value,
    };
    let mode = Value::Enum {
        variant: "Override".into(),
        payload: Box::new(Value::Struct(Default::default())),
    };
    let mask = Value::List(vec![Value::String("Spine".into())]);
    let parameters = Value::Map([("speed/main".into(), Value::F64(0.0))].into());
    let parameter = r#"layers[0].parameters["speed/main"]"#;
    let plan = world.plan(vec![
        set("layers", Value::List(vec![layer.default])),
        set("layers[0].mode", mode),
        set("layers[0].weight", Value::F64(0.5)),
        set("layers[0].mask", mask),
        set("layers[0].parameters", parameters),
        set(parameter, Value::F64(2.0)),
        set("layers[0].time", Value::F64(0.25)),
    ]);
    assert!(plan.is_valid(), "{:?}", plan.diagnostics);
    let applied = world.apply(&plan).unwrap();
    let hash = world.state_hash();
    let data = world.world_data();
    let mut reopened = World::new();
    reopened.restore_world_data(&data).unwrap();
    assert_eq!(reopened.state_hash(), hash);
    assert_eq!(
        reopened.get_field(entity, AnimationLayers::ID, parameter),
        Some(Value::F64(2.0))
    );
    assert_eq!(
        reopened.get_field(entity, AnimationLayers::ID, "layers[0].mask[0]"),
        Some(Value::String("Spine".into()))
    );
    assert_eq!(
        reopened.get_field(entity, AnimationLayers::ID, "layers[0].time"),
        Some(Value::F64(0.25))
    );

    let invalid = world.plan(vec![set(parameter, Value::F64(f64::MAX))]);
    assert!(!invalid.is_valid());
    assert!(world.apply(&invalid).is_err());
    assert_eq!(world.world_data(), data);
    world.revert(&applied).unwrap();
    assert_eq!(
        world.get_field(entity, AnimationLayers::ID, "layers"),
        Some(Value::List(Vec::new()))
    );
}
