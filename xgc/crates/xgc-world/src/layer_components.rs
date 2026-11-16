//! 动画叠加层组件：RON `[(clip: "...", weight: 0.5), ...]`。

use xgc_reflect::{Component, ComponentId, FieldDescriptor, FieldType, Value};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnimationLayers {
    pub layers: String,
}

impl Component for AnimationLayers {
    const ID: ComponentId = ComponentId(0x0506);
    const NAME: &'static str = "animation.layers";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "动画叠加层：按权重把多个剪辑混合到基础姿态之上"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new("layers", FieldType::String, Value::String(String::new()))
                .doc("RON `[(clip, weight)]`；空 = 不启用"),
        ]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        match field {
            "layers" => Some(Value::String(self.layers.clone())),
            _ => None,
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        match field {
            "layers" => match value {
                Value::String(value) => {
                    self.layers = value.clone();
                    Ok(())
                }
                other => Err(format!(
                    "animation.layers.layers 期望 string，得到 {}",
                    other.type_name()
                )),
            },
            _ => Err(format!("animation.layers 没有字段 {field}")),
        }
    }
}
