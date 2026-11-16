//! LOD 组件：层级快照（RON 文本）。

use xgc_reflect::{Component, ComponentId, FieldDescriptor, FieldType, Value};

#[derive(Clone, Debug, PartialEq, Default)]
pub struct LodGroup {
    pub levels: String,
}

impl Component for LodGroup {
    const ID: ComponentId = ComponentId(0x0204);
    const NAME: &'static str = "render.lod";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "LOD 层级快照：RON 数组，元素为 { model, max_distance }"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new("levels", FieldType::String, Value::String(String::new()))
                .doc("层级 RON；空 = 无 LOD"),
        ]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        match field {
            "levels" => Some(Value::String(self.levels.clone())),
            _ => None,
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        match field {
            "levels" => {
                self.levels = value
                    .as_string()
                    .ok_or_else(|| {
                        format!("render.lod.levels 期望 string，得到 {}", value.type_name())
                    })?
                    .to_string();
                Ok(())
            }
            _ => Err(format!("render.lod 没有字段 {field}")),
        }
    }
}
