//! 动画状态机组件：机器 / 当前状态 / 状态时间 / 数值参数。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
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

/// 动画状态机的机器定义（RON 字符串的强类型视图）。
///
/// 放在 world 而不是 sim：迁移器（`xgc-migrate`）要生成它、运行时要解析它，
/// 两边共用一个类型才不会各自漂移。旧端的机器是 `entry`/`states`/`transitions`
/// 三段，转移条件是 `AtLeast` / `Below` 两种；这里保留同一概念，只把字段名对齐
/// XGC 的 RON 写法。
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct StateMachine {
    pub initial: String,
    #[serde(default)]
    pub states: Vec<StateDef>,
    #[serde(default)]
    pub transitions: Vec<TransitionDef>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct StateDef {
    pub name: String,
    pub clip: String,
    #[serde(default = "default_looping")]
    pub looping: bool,
    #[serde(default = "default_speed")]
    pub speed: f32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct TransitionDef {
    #[serde(default)]
    pub from: Option<String>,
    pub to: String,
    /// 触发条件；`None` = 无条件。
    #[serde(default)]
    pub when: Option<TransitionCondition>,
    /// 过渡时长（秒）；`0` = 用 `Animator.fade_time` 的缺省值。
    #[serde(default)]
    pub duration: f32,
}

/// 转移条件：旧引擎 `AtLeast` / `Below` 的同一概念。
///
/// 两条都要保留：`AtLeast` 管"够快就切"，`Below` 管回退与滞回；只有一半时
/// 会在阈值附近来回抖。
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub enum TransitionCondition {
    AtLeast { parameter: String, value: f32 },
    Below { parameter: String, value: f32 },
}

impl TransitionCondition {
    /// 参数表里缺的键按 0.0 处理——与旧运行时一致（没写过的参数视为 0）。
    pub fn matches(&self, parameters: &BTreeMap<String, f32>) -> bool {
        let (parameter, value) = match self {
            Self::AtLeast { parameter, value } | Self::Below { parameter, value } => {
                (parameter, *value)
            }
        };
        let current = parameters.get(parameter).copied().unwrap_or(0.0);
        match self {
            Self::AtLeast { .. } => current >= value,
            Self::Below { .. } => current < value,
        }
    }
}

pub fn parse_machine(text: &str) -> Result<StateMachine, String> {
    ron::from_str(text).map_err(|err| format!("状态机解析失败：{err}"))
}

pub fn parse_parameters(text: &str) -> BTreeMap<String, f32> {
    ron::from_str(text).unwrap_or_default()
}

fn default_looping() -> bool {
    true
}

fn default_speed() -> f32 {
    1.0
}
