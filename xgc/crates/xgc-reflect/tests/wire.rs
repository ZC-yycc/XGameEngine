use serde_json::json;
use xgc_reflect::{FieldType, Value};

#[test]
fn tagged_recursive_values_round_trip() {
    let wire = json!({"kind": "list", "value": [{"kind": "struct", "value": {
        "name": {"kind": "string", "value": "hit"},
        "time": {"kind": "f64", "value": 0.25}
    }}]});
    let value: Value = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(value.kind(), "list");
    assert_eq!(value.type_name(), "list");
    assert_eq!(serde_json::to_value(value).unwrap(), wire);
}

#[test]
fn scalar_wire_tags_and_scalar_fits_are_preserved() {
    for (value, kind, expected) in [
        (Value::Bool(true), "bool", json!(true)),
        (Value::I64(-2), "i64", json!(-2)),
        (Value::F64(0.25), "f64", json!(0.25)),
        (Value::String("data".into()), "string", json!("data")),
        (Value::Vec2([1.0, 0.0]), "vec2", json!([1.0, 0.0])),
        (Value::Vec3([1.0, 0.0, 0.0]), "vec3", json!([1.0, 0.0, 0.0])),
        (
            Value::Vec4([1.0, 0.0, 0.0, 0.0]),
            "vec4",
            json!([1.0, 0.0, 0.0, 0.0]),
        ),
        (
            Value::Quat([1.0, 0.0, 0.0, 0.0]),
            "quat",
            json!([1.0, 0.0, 0.0, 0.0]),
        ),
        (Value::Entity(2), "entity", json!(2)),
    ] {
        let wire = json!({"kind": kind, "value": expected});
        assert_eq!(serde_json::to_value(&value).unwrap(), wire);
        assert_eq!(serde_json::from_value::<Value>(wire).unwrap(), value);
        assert_eq!(value.kind(), kind);
        assert_eq!(value.type_name(), kind);
        assert!(value.fits(&FieldType::parse(kind).unwrap()));
    }
    assert!(Value::I64(i64::from(u32::MAX)).fits(&FieldType::U32));
    assert!(!Value::I64(i64::from(u32::MAX) + 1).fits(&FieldType::U32));
    assert!(!Value::I64(-1).fits(&FieldType::U32));
    assert!(FieldType::parse("list").is_none());
    assert!(FieldType::parse("struct").is_none());
}

#[test]
fn recursive_hash_distinguishes_container_types_and_nested_values() {
    fn digest(value: &Value) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325;
        value.hash_into(&mut hash);
        hash
    }

    assert_ne!(
        digest(&Value::List(vec![])),
        digest(&Value::Struct(Default::default()))
    );
    assert_ne!(
        digest(&Value::List(vec![Value::I64(1)])),
        digest(&Value::List(vec![Value::Bool(true)]))
    );
    let a = Value::Struct([("a".into(), Value::I64(1)), ("b".into(), Value::Bool(true))].into());
    let b = Value::Struct([("b".into(), Value::Bool(true)), ("a".into(), Value::I64(1))].into());
    assert_eq!(digest(&a), digest(&b));
}
