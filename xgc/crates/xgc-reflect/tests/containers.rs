use serde_json::json;
use xgc_reflect::{FieldDescriptor, FieldPath, FieldSchema, FieldType, Value};

fn condition(variant: &str, limit: Value) -> Value {
    Value::Enum {
        variant: variant.into(),
        payload: Box::new(Value::Struct([("limit".into(), limit)].into())),
    }
}

#[test]
fn map_and_enum_share_paths_between_values_and_schema() {
    let limit = FieldDescriptor::new("limit", FieldType::F32, Value::F64(0.0));
    let below = FieldDescriptor::new(
        "Below",
        FieldType::Struct(vec![limit]),
        Value::Struct([("limit".into(), Value::F64(0.0))].into()),
    );
    let always = FieldDescriptor::new(
        "Always",
        FieldType::Struct(Vec::new()),
        Value::Struct(Default::default()),
    );
    let ty = FieldType::Map(Box::new(FieldDescriptor::new(
        "rule",
        FieldType::Enum(vec![below, always]),
        condition("Below", Value::F64(0.0)),
    )));
    let key = "行走.慢[1]\"\\\n";
    let mut value = Value::Map([(key.to_string(), condition("Below", Value::F64(1.0)))].into());
    let schema = FieldSchema::from(FieldDescriptor::new("rules", ty.clone(), value.clone()));
    assert!(value.fits(&ty));
    assert!(schema.fits(&value));
    let path =
        FieldPath::parse(&format!("rules{}.Below.limit", FieldPath::key_segment(key))).unwrap();
    assert_eq!(path.schema(&schema).unwrap().ty, "f32");
    assert_eq!(path.read(&value), Ok(&Value::F64(1.0)));
    assert_eq!(
        path.replace(&mut value, Value::F64(2.0)),
        Ok(Value::F64(1.0))
    );
    let inactive =
        FieldPath::parse(&format!("rules{}.Always", FieldPath::key_segment(key))).unwrap();
    assert!(inactive.schema(&schema).is_ok());
    assert!(inactive.read(&value).is_err());
    let before = value.clone();
    assert!(inactive.replace(&mut value, Value::Bool(false)).is_err());
    assert!(FieldPath::parse(r#"rules["typo"]"#)
        .unwrap()
        .replace(&mut value, Value::Bool(false))
        .is_err());
    assert_eq!(value, before);
    for invalid in [
        condition("Unknown", Value::F64(1.0)),
        condition("Below", Value::I64(1)),
    ] {
        let invalid = Value::Map([(key.into(), invalid)].into());
        assert!(!invalid.fits(&ty));
        assert!(!schema.fits(&invalid));
    }
    let wire = serde_json::to_value(&value).unwrap();
    assert_eq!(
        wire,
        json!({"kind": "map", "value": {key: {"kind": "enum", "value": {
            "variant": "Below", "payload": {"kind": "struct", "value": {"limit": {"kind": "f64", "value": 2.0}}}
        }}}})
    );
    assert_eq!(serde_json::from_value::<Value>(wire).unwrap(), value);
}

#[test]
fn map_keys_use_json_escaping_and_require_a_bracket() {
    for invalid in [
        r#"rules["broken]"#,
        r#"rules["key" ]"#,
        r#"rules["key"]suffix"#,
        r#"rules["\q"]"#,
        r#"rules["key""#,
    ] {
        assert!(FieldPath::parse(invalid).is_err(), "{invalid}");
    }
    let values = Value::Map(
        [
            ("".into(), Value::Bool(true)),
            ("7".into(), Value::F64(7.0)),
        ]
        .into(),
    );
    assert_eq!(
        FieldPath::parse(r#"rules[""]"#).unwrap().read(&values),
        Ok(&Value::Bool(true))
    );
    assert_eq!(
        FieldPath::parse(r#"rules["7"]"#).unwrap().read(&values),
        Ok(&Value::F64(7.0))
    );
    assert!(FieldPath::parse("rules[7]").unwrap().read(&values).is_err());
}

#[test]
fn recursive_hash_distinguishes_maps_structs_and_enum_variants() {
    let fields = std::collections::BTreeMap::from([("speed".into(), Value::F64(1.0))]);
    let hash = |value: Value| {
        let mut hash = 0xcbf29ce484222325;
        value.hash_into(&mut hash);
        hash
    };
    assert_ne!(
        hash(Value::Map(fields.clone())),
        hash(Value::Struct(fields))
    );
    assert_ne!(
        hash(condition("Below", Value::F64(1.0))),
        hash(condition("AtLeast", Value::F64(1.0)))
    );
}
