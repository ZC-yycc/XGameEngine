//! 复合字段走同一个事务入口，路径错误不会新增键或切换枚举分支。

use xgc_base::diagnostics as codes;
use xgc_reflect::{FieldDescriptor, FieldType, Registry};
use xgc_world::{Command, Component, ComponentId, Value, World};

#[derive(Clone)]
struct Rules(Value);

fn rule(variant: &str, value: f64) -> Value {
    Value::Enum {
        variant: variant.into(),
        payload: Box::new(Value::Struct([("limit".into(), Value::F64(value))].into())),
    }
}

fn rules(key: &str, value: Value) -> Value {
    Value::Map([(key.into(), value)].into())
}

impl Component for Rules {
    const ID: ComponentId = ComponentId(0xffff);
    const NAME: &'static str = "test.rules";

    fn default_value() -> Self {
        Self(rules("speed.0", rule("Below", 0.25)))
    }

    fn fields() -> Vec<FieldDescriptor> {
        let variant = |name| {
            FieldDescriptor::new(
                name,
                FieldType::Struct(vec![FieldDescriptor::new(
                    "limit",
                    FieldType::F32,
                    Value::F64(0.0),
                )
                .range(0.0, 1.0)]),
                Value::Struct([("limit".into(), Value::F64(0.0))].into()),
            )
        };
        vec![FieldDescriptor::new(
            "rules",
            FieldType::Map(Box::new(FieldDescriptor::new(
                "rule",
                FieldType::Enum(vec![variant("Below"), variant("AtLeast")]),
                rule("Below", 0.0),
            ))),
            Self::default_value().0,
        )]
    }

    fn get_field(&self, name: &str) -> Option<Value> {
        (name == "rules").then(|| self.0.clone())
    }

    fn set_field(&mut self, name: &str, value: &Value) -> Result<(), String> {
        if name != "rules" || !value.fits(&Self::fields()[0].ty) {
            return Err("rules 类型无效".into());
        }
        self.0 = value.clone();
        Ok(())
    }
}

#[test]
fn switches_variants_edits_and_reverts_through_escaped_map_paths() {
    let mut world = World::new();
    world.register_component::<Rules>();
    let id = world.spawn((Rules::default_value(),));
    let entity = world.entity(id).unwrap();
    let set = |field: &str, value| Command::SetField {
        entity,
        component: Rules::ID,
        field: field.into(),
        value,
    };
    let before = world.world_data().entities[0].components.clone();
    let plan = world.plan(vec![
        set(r#"rules["speed.0"]"#, rule("AtLeast", 0.5)),
        set(r#"rules["speed.0"].AtLeast.limit"#, Value::F64(0.75)),
    ]);
    let applied = world.apply(&plan).unwrap();
    assert_eq!(
        world.get_field(entity, Rules::ID, r#"rules["speed.0"].AtLeast.limit"#),
        Some(Value::F64(0.75))
    );
    let active = world.world_data();
    let invalid = world.plan(vec![
        set(r#"rules["speed.0"].AtLeast.limit"#, Value::F64(0.5)),
        set(r#"rules["speed.0"].Below.limit"#, Value::F64(0.4)),
    ]);
    assert_eq!(invalid.diagnostics[0].code, codes::EDIT_UNKNOWN_FIELD);
    assert!(world.apply(&invalid).is_err());
    assert_eq!(world.world_data(), active);
    world.revert(&applied).unwrap();
    assert_eq!(world.world_data().entities[0].components, before);
}

#[test]
fn validation_reports_a_range_error_at_the_exact_container_path() {
    let key = "dot.\"quote\"]";
    let mut world = World::new();
    world.spawn((Rules(rules(key, rule("Below", 2.0))),));
    let mut registry = Registry::new();
    registry.register::<Rules>().unwrap();
    let issues = world.validate(&registry);
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].code, codes::WORLD_RANGE);
    assert!(issues[0]
        .subject
        .to_string()
        .contains(r#"rules["dot.\"quote\"]"].Below.limit"#));
}
