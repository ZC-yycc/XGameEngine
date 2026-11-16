//! Rust 插件组件：注册名 + 字段快照（RON 文本）。

use xgc_reflect::{Component, ComponentId, FieldDescriptor, FieldType, Value};

#[derive(Clone, Debug, PartialEq, Default)]
pub struct PluginComponent {
    pub registered_name: String,
    pub fields: String,
}

impl Component for PluginComponent {
    const ID: ComponentId = ComponentId(0x0801);
    const NAME: &'static str = "plugin.rust";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "Rust 插件组件：注册名 + 字段快照（RON 文本，空 = 无字段）"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new(
                "registered_name",
                FieldType::String,
                Value::String(String::new()),
            )
            .doc("插件注册名"),
            FieldDescriptor::new("fields", FieldType::String, Value::String(String::new()))
                .doc("字段快照 RON；空 = None"),
        ]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        match field {
            "registered_name" => Some(Value::String(self.registered_name.clone())),
            "fields" => Some(Value::String(self.fields.clone())),
            _ => None,
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        match field {
            "registered_name" => {
                self.registered_name = value
                    .as_string()
                    .ok_or_else(|| {
                        format!(
                            "plugin.rust.registered_name 期望 string，得到 {}",
                            value.type_name()
                        )
                    })?
                    .to_string();
                Ok(())
            }
            "fields" => {
                self.fields = value
                    .as_string()
                    .ok_or_else(|| {
                        format!("plugin.rust.fields 期望 string，得到 {}", value.type_name())
                    })?
                    .to_string();
                Ok(())
            }
            _ => Err(format!("plugin.rust 没有字段 {field}")),
        }
    }
}
