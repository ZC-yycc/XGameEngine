use xgc_reflect::{FieldPath, Value};

fn timeline() -> Value {
    Value::List(vec![Value::Struct(
        [
            ("time".into(), Value::F64(0.25)),
            ("name".into(), Value::String("hit".into())),
        ]
        .into(),
    )])
}

#[test]
fn nested_paths_read_and_replace_existing_slots() {
    let mut root_value = timeline();
    let time = FieldPath::parse("events[0].time").unwrap();
    let name = FieldPath::parse("events[0].name").unwrap();
    assert_eq!(time.root(), "events");
    assert!(!time.is_root());
    assert_eq!(time.read(&root_value), Ok(&Value::F64(0.25)));
    assert_eq!(
        time.replace(&mut root_value, Value::F64(0.5)),
        Ok(Value::F64(0.25))
    );
    assert_eq!(time.read(&root_value), Ok(&Value::F64(0.5)));
    assert_eq!(name.read(&root_value), Ok(&Value::String("hit".into())));

    let root = FieldPath::parse("events").unwrap();
    assert_eq!(root.root(), "events");
    assert!(root.is_root());
    assert_eq!(root.read(&root_value), Ok(&root_value));
    let previous = root_value.clone();
    let empty = Value::List(Vec::new());
    assert_eq!(root.replace(&mut root_value, empty.clone()), Ok(previous));
    assert_eq!(root_value, empty);
}

#[test]
fn malformed_paths_are_rejected() {
    for path in [
        "",
        ".events",
        "[0]",
        "events.",
        "events..time",
        "events.[0]",
        "events[]",
        "events[-1]",
        "events[+1]",
        "events[ 1]",
        "events[1 ]",
        "events[0",
        "events]",
        "events[0]]",
        "events[0]time",
        "events[0].",
        " events",
        "events ",
        "events. time",
        "events[9999999999999999999999999999999]",
    ] {
        assert!(FieldPath::parse(path).is_err(), "accepted {path:?}");
    }
}

#[test]
fn failed_traversal_leaves_the_value_untouched() {
    let original = timeline();
    let mut root = original.clone();
    for path in [
        "events[1]",
        "events.time",
        "events[0].missing",
        "events[0][0]",
        "events[\"name\"]",
        "events[0].time.x",
    ] {
        let path = FieldPath::parse(path).unwrap();
        assert!(path.read(&root).is_err());
        assert!(path.replace(&mut root, Value::Bool(false)).is_err());
        assert_eq!(root, original);
    }
}

#[test]
fn consecutive_indices_traverse_nested_lists() {
    let root = Value::List(vec![Value::List(vec![Value::I64(42)])]);
    assert_eq!(
        FieldPath::parse("grid[0][0]").unwrap().read(&root),
        Ok(&Value::I64(42))
    );
}
