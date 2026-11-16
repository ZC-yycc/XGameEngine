//! 动画状态机组件：机器 / 当前状态 / 状态时间 / 数值参数。

use xgc_reflect::{Component, ComponentId, FieldDescriptor, FieldType, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct AnimationState {
    pub machine: String,
    pub state: String,
    pub time: f32,
    pub parameters: String,
}

impl Default for AnimationState {
    fn default() -> Self {
        Self {
            machine: String::new(),
            state: String::new(),
            time: 0.0,
            parameters: "{}".to_string(),
        }
    }
}

impl Component for AnimationState {
    const ID: ComponentId = ComponentId(0x0505);
    const NAME: &'static str = "animation.state";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "动画状态机：RON 机器 / 当前状态 / 状态时间 / RON 数值参数"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new("machine", FieldType::String, Value::String(String::new()))
                .doc("状态机 RON；空 = 不启用"),
            FieldDescriptor::new("state", FieldType::String, Value::String(String::new()))
                .doc("当前状态名；空 = 使用机器 initial"),
            FieldDescriptor::new("time", FieldType::F32, Value::F64(0.0))
                .doc("当前状态已持续时间（秒）"),
            FieldDescriptor::new(
                "parameters",
                FieldType::String,
                Value::String("{}".to_string()),
            )
            .doc("数值参数 RON map<String, f32>"),
        ]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        match field {
            "machine" => Some(Value::String(self.machine.clone())),
            "state" => Some(Value::String(self.state.clone())),
            "time" => Some(Value::F64(f64::from(self.time))),
            "parameters" => Some(Value::String(self.parameters.clone())),
            _ => None,
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        match field {
            "machine" => self.machine = string(value, field)?,
            "state" => self.state = string(value, field)?,
            "time" => self.time = number(value, field)?,
            "parameters" => self.parameters = string(value, field)?,
            _ => return Err(format!("animation.state 没有字段 {field}")),
        }
        Ok(())
    }
}

fn string(value: &Value, field: &str) -> Result<String, String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        other => Err(format!(
            "animation.state.{field} 期望 string，得到 {}",
            other.type_name()
        )),
    }
}

fn number(value: &Value, field: &str) -> Result<f32, String> {
    match value {
        Value::F64(value) => Ok(*value as f32),
        other => Err(format!(
            "animation.state.{field} 期望 number，得到 {}",
            other.type_name()
        )),
    }
}
