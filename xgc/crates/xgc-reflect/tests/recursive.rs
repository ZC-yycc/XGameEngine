use serde_json::json;
use xgc_reflect::{Component, ComponentId, FieldDescriptor, FieldPath, FieldType, Registry, Value};

#[derive(Clone)]
struct Timeline;

impl Component for Timeline {
    const ID: ComponentId = ComponentId(0x0100);
    const NAME: &'static str = "test.timeline";

    fn default_value() -> Self {
        Self
    }

    fn fields() -> Vec<FieldDescriptor> {
        let time = FieldDescriptor::new("time", FieldType::F32, Value::F64(0.0))
            .unit("s")
            .range(0.0, 60.0)
            .step(0.05)
            .group("Timing")
            .read_only()
            .doc("事件时刻");
        let name = FieldDescriptor::new("name", FieldType::String, Value::String("marker".into()));
        let event = FieldDescriptor::new(
            "event",
            FieldType::Struct(vec![time, name]),
            event(Value::F64(0.0), Value::String("marker".into())),
        );
        vec![FieldDescriptor::new(
            "events",
            FieldType::List(Box::new(event)),
            Value::List(Vec::new()),
        )]
    }
}

fn event(time: Value, name: Value) -> Value {
    Value::Struct([("time".into(), time), ("name".into(), name)].into())
}

#[test]
fn recursive_schema_exposes_the_insertion_default_and_leaf_metadata() {
    let mut registry = Registry::new();
    registry.register::<Timeline>().unwrap();
    assert!(registry.validate().is_empty());

    let events = &registry.get(Timeline::ID).unwrap().fields[0];
    assert_eq!(events.ty, "list");
    assert_eq!(events.default, Value::List(Vec::new()));
    assert!(events.required && events.fields.is_empty());
    let element = events.element.as_ref().unwrap();
    assert_eq!(element.ty, "struct");
    assert_eq!(
        element.default,
        event(Value::F64(0.0), Value::String("marker".into()))
    );
    assert!(element.required && element.element.is_none());
    assert_eq!(element.fields.len(), 2);
    assert_eq!(
        serde_json::to_value(&element.fields[0]).unwrap(),
        json!({
            "name": "time", "ty": "f32", "default": {"kind": "f64", "value": 0.0},
            "required": true, "unit": "s", "min": 0.0, "max": 60.0,
            "step": 0.05, "read_only": true, "group": "Timing", "doc": "事件时刻"
        })
    );
}

#[test]
fn fits_requires_declared_fields_and_validates_every_list_entry() {
    let ty = &Timeline::fields()[0].ty;
    let good = event(Value::F64(1.0), Value::String("hit".into()));
    assert!(Value::List(Vec::new()).fits(ty));
    assert!(Value::List(vec![good.clone()]).fits(ty));
    assert!(!good.fits(ty));
    assert!(!Value::List(vec![good.clone(), Value::Bool(false)]).fits(ty));
    assert!(!Value::List(vec![event(Value::I64(1), Value::String("hit".into()))]).fits(ty));
    let missing = Value::Struct([("time".into(), Value::F64(1.0))].into());
    assert!(!Value::List(vec![missing]).fits(ty));
    let Value::Struct(mut fields) = good else {
        unreachable!()
    };
    fields.insert("extra".into(), Value::Bool(false));
    assert!(!Value::List(vec![Value::Struct(fields)]).fits(ty));
}

#[test]
fn schema_lookup_resolves_nested_paths_and_rejects_impossible_steps() {
    let mut registry = Registry::new();
    registry.register::<Timeline>().unwrap();
    let events = &registry.get(Timeline::ID).unwrap().fields[0];
    let path = FieldPath::parse("events[99].time").unwrap();
    let time = path.schema(events).unwrap();
    assert_eq!(time.ty, "f32");
    assert_eq!(time.step, Some(0.05));
    assert!(time.read_only);
    assert_eq!(time.group.as_deref(), Some("Timing"));
    assert!(FieldPath::parse("events[0].missing")
        .unwrap()
        .schema(events)
        .is_err());
    assert!(FieldPath::parse("events.name")
        .unwrap()
        .schema(events)
        .is_err());
    assert!(events.fits(&Value::List(vec![event(
        Value::F64(1.0),
        Value::String("hit".into())
    )])));
    assert!(!events.fits(&Value::List(vec![event(
        Value::I64(1),
        Value::String("hit".into())
    )])));
}

#[derive(Clone)]
struct BrokenElement;

impl Component for BrokenElement {
    const ID: ComponentId = ComponentId(0x0101);
    const NAME: &'static str = "test.broken_element";

    fn default_value() -> Self {
        Self
    }

    fn fields() -> Vec<FieldDescriptor> {
        let mut fields = Timeline::fields();
        let FieldType::List(element) = &mut fields[0].ty else {
            unreachable!()
        };
        element.default = event(Value::Bool(false), Value::String("marker".into()));
        fields
    }
}

#[test]
fn invalid_insertion_default_is_reported_even_for_an_empty_list() {
    let mut registry = Registry::new();
    registry.register::<BrokenElement>().unwrap();
    let diagnostics = registry.validate();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, "SCHEMA.DEFAULT_TYPE");
}
