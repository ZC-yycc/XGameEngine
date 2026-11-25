use super::*;
use xgc_reflect::{Component, FieldDescriptor, FieldType, Value};

#[derive(Clone)]
struct Settings(Value);

impl Component for Settings {
    const ID: ComponentId = ComponentId(0xffff);
    const NAME: &'static str = "test.settings";

    fn default_value() -> Self {
        let value = Value::Enum {
            variant: "Number".into(),
            payload: Box::new(Value::F64(1.0)),
        };
        Self(Value::Map([("gain:main".into(), value)].into()))
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![FieldDescriptor::new(
            "values",
            FieldType::Map(Box::new(FieldDescriptor::new(
                "entry",
                FieldType::Enum(vec![FieldDescriptor::new(
                    "Number",
                    FieldType::F64,
                    Value::F64(0.0),
                )]),
                Value::Enum {
                    variant: "Number".into(),
                    payload: Box::new(Value::F64(0.0)),
                },
            ))),
            Self::default_value().0,
        )]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        (field == "values").then(|| self.0.clone())
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        if field != "values" || !value.fits(&Self::fields()[0].ty) {
            return Err("values 类型错误".into());
        }
        self.0 = value.clone();
        Ok(())
    }
}

#[test]
fn protocol_queries_edits_and_reverts_a_map_enum_leaf() {
    let mut agent = Agent::new();
    agent.registry.register::<Settings>().unwrap();
    agent.world.register_component::<Settings>();
    let entity = agent.world.spawn((Settings::default_value(),));
    let id = agent.world.entity(entity).unwrap();
    let field = r#"values["gain:main"].Number"#;
    let path = format!("test.settings:{field}");
    let missing = r#"test.settings:values["absent"].Number"#;
    let query = |agent: &mut Agent, value| {
        agent.call("world.query", &json!({"select": [path, missing], "filter": {"path":path,"op":"eq","value":value}})).unwrap()
    };
    let before = query(&mut agent, 1.0);
    assert_eq!(before["total"], 1);
    assert_eq!(
        before["rows"][0]["values"][&path],
        json!({"kind":"f64","value":1.0})
    );
    assert_eq!(before["rows"][0]["values"].get(missing), Some(&Json::Null));
    let plan = agent
        .call(
            "edit.plan",
            &json!({"commands": [Command::SetField {
                entity: id,
                component: Settings::ID,
                field: field.into(),
                value: Value::F64(2.0),
            }]}),
        )
        .unwrap();
    let tx = agent
        .call("edit.apply", &json!({"plan_id": plan["plan_id"]}))
        .unwrap();
    assert_eq!(query(&mut agent, 2.0)["total"], 1);
    agent
        .call("edit.revert", &json!({"tx_id": tx["tx_id"]}))
        .unwrap();
    assert_eq!(query(&mut agent, 1.0)["rows"], before["rows"]);
}
