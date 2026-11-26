use std::collections::BTreeMap;

use xgc_reflect::{Component, ComponentId, FieldType, ReflectValue, Registry, Value};

#[derive(Clone, Debug, Default, PartialEq, ReflectValue)]
#[reflect(element_name = "level")]
struct Level {
    model: String,
    #[field(unit = "m", range = (-10.0, 20.0), step = 0.25, doc = "距离")]
    distance: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Component)]
#[component(id = 0x3200, name = "test.levels", doc = "测试组件")]
struct Levels {
    #[field(doc = "有序层级")]
    levels: Vec<Level>,
    #[field(group = "State", read_only, skip_hash)]
    settings: BTreeMap<String, u32>,
}

#[test]
fn derives_a_single_ordered_schema_from_rust_fields() {
    let mut registry = Registry::new();
    registry.register::<Levels>().unwrap();
    assert_eq!(Levels::ID, ComponentId(0x3200));
    assert_eq!(Levels::NAME, "test.levels");
    assert_eq!(Levels::doc(), "测试组件");
    let fields = Levels::fields();
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].name, "levels");
    assert_eq!(fields[0].doc, "有序层级");
    let FieldType::List(element) = &fields[0].ty else {
        panic!("应生成列表描述");
    };
    assert_eq!(element.name, "level");
    assert_eq!(element.default, Level::default().to_value());
    let FieldType::Struct(children) = &element.ty else {
        panic!("应生成结构体描述");
    };
    assert_eq!(children[0].name, "model");
    assert_eq!(children[1].name, "distance");
    assert_eq!(children[1].ty, FieldType::F32);
    assert_eq!(children[1].unit, Some("m"));
    assert_eq!(children[1].range, Some((-10.0, 20.0)));
    assert_eq!(children[1].step, Some(0.25));
    assert_eq!(children[1].doc, "距离");
    assert!(fields[1].read_only);
    assert_eq!(fields[1].group, Some("State"));
    assert_eq!(Levels::state_fields(), vec![fields[0].clone()]);
}

#[test]
fn rejects_invalid_nested_values_without_partial_assignment() {
    let mut levels = Levels::default_value();
    let level = Level {
        model: "cube".into(),
        distance: -1.0,
    };
    let valid_level = level.to_value();
    levels
        .set_field("levels", &Value::List(vec![valid_level.clone()]))
        .unwrap();
    assert_eq!(levels.levels, vec![level]);
    let before = levels.clone();
    let Value::Struct(mut malformed) = valid_level.clone() else {
        unreachable!()
    };
    malformed.insert("distance".into(), Value::F64(f64::from(f32::MAX) * 2.0));
    let error = levels
        .set_field(
            "levels",
            &Value::List(vec![valid_level.clone(), Value::Struct(malformed.clone())]),
        )
        .unwrap_err();
    assert!(
        error.contains("[1]") && error.contains("distance"),
        "{error}"
    );
    assert_eq!(levels, before);

    malformed.remove("distance");
    malformed.insert("other".into(), Value::F64(1.0));
    assert!(Level::from_value(&Value::Struct(malformed)).is_err());
    assert!(levels.set_field("unknown", &Value::Bool(true)).is_err());
    assert_eq!(levels.get_field("unknown"), None);

    let settings = Value::Map(BTreeMap::from([("quality".into(), Value::I64(3))]));
    levels.set_field("settings", &settings).unwrap();
    assert_eq!(levels.get_field("settings"), Some(settings.clone()));
    let invalid = Value::Map(BTreeMap::from([("quality".into(), Value::I64(-1))]));
    assert!(levels.set_field("settings", &invalid).is_err());
    assert_eq!(levels.get_field("settings"), Some(settings));
}
