//! 动画状态机组件：机器定义、当前状态、状态时间和数值参数。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use xgc_reflect::{Component, ComponentId, FieldDescriptor, FieldType, Value};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnimationState {
    pub machine: StateMachine,
    pub state: String,
    pub time: f32,
    pub parameters: BTreeMap<String, f32>,
}

impl Component for AnimationState {
    const ID: ComponentId = ComponentId(0x0505);
    const NAME: &'static str = "animation.state";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "动画状态机定义、当前状态、状态时间和数值参数"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            StateMachine::descriptor().doc("空机器不驱动 Animator"),
            text_field("state").doc("当前状态名；空表示从机器入口开始"),
            number_field("time", 0.0)
                .unit("s")
                .doc("当前状态已持续时间"),
            FieldDescriptor::new(
                "parameters",
                FieldType::Map(Box::new(number_field("parameter", 0.0))),
                Value::Map(BTreeMap::new()),
            )
            .doc("按名字保存的状态机参数；条件中未设定的参数按零处理"),
        ]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        match field {
            "machine" => Some(self.machine.to_value()),
            "state" => Some(text_value(&self.state)),
            "time" => Some(number_value(self.time)),
            "parameters" => Some(Value::Map(
                self.parameters
                    .iter()
                    .map(|(key, value)| (key.clone(), number_value(*value)))
                    .collect(),
            )),
            _ => None,
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        match field {
            "machine" => self.machine = StateMachine::from_value(value)?,
            "state" => self.state = text(value, "state")?,
            "time" => self.time = number(value, "time")?,
            "parameters" => {
                let Value::Map(entries) = value else {
                    return Err("parameters 必须是字符串键映射".into());
                };
                self.parameters = entries
                    .iter()
                    .map(|(key, value)| number(value, key).map(|value| (key.clone(), value)))
                    .collect::<Result<_, _>>()?;
            }
            _ => return Err(format!("animation.state 没有字段 {field}")),
        }
        Ok(())
    }
}

/// 基础状态机与混合层共用的机器定义。
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
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

impl Default for StateDef {
    fn default() -> Self {
        Self {
            name: String::new(),
            clip: String::new(),
            looping: default_looping(),
            speed: default_speed(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub enum TransitionCondition {
    AtLeast { parameter: String, value: f32 },
    Below { parameter: String, value: f32 },
}

impl TransitionCondition {
    /// 参数表里缺的键按 0.0 处理。
    pub fn matches(&self, parameters: &BTreeMap<String, f32>) -> bool {
        match self {
            Self::AtLeast { parameter, value } => {
                parameters.get(parameter).copied().unwrap_or(0.0) >= *value
            }
            Self::Below { parameter, value } => {
                parameters.get(parameter).copied().unwrap_or(0.0) < *value
            }
        }
    }
}

impl StateMachine {
    pub fn is_empty(&self) -> bool {
        self.initial.is_empty() && self.states.is_empty() && self.transitions.is_empty()
    }

    /// 混合层的机器只有在入口状态存在时才覆盖直接剪辑。
    pub fn is_usable(&self) -> bool {
        !self.initial.is_empty() && self.state_def(&self.initial).is_some()
    }

    pub fn state_def(&self, name: &str) -> Option<&StateDef> {
        self.states.iter().find(|state| state.name == name)
    }

    /// 所有宿主取第一条命中的转移，配置顺序决定优先级。
    pub fn transition_for<'a>(
        &'a self,
        current: &str,
        parameters: &BTreeMap<String, f32>,
    ) -> Option<&'a TransitionDef> {
        self.transitions.iter().find(|transition| {
            transition
                .from
                .as_deref()
                .is_none_or(|from| from == current)
                && transition
                    .when
                    .as_ref()
                    .is_none_or(|when| when.matches(parameters))
        })
    }
}

impl StateMachine {
    fn descriptor() -> FieldDescriptor {
        FieldDescriptor::new(
            "machine",
            FieldType::Struct(vec![
                text_field("initial").doc("入口状态名"),
                list_field("states", StateDef::descriptor()).doc("机器可播放的状态"),
                list_field("transitions", TransitionDef::descriptor()).doc("按优先级排列的转移"),
            ]),
            Self::default().to_value(),
        )
    }

    fn to_value(&self) -> Value {
        record([
            ("initial", text_value(&self.initial)),
            (
                "states",
                Value::List(self.states.iter().map(StateDef::to_value).collect()),
            ),
            (
                "transitions",
                Value::List(
                    self.transitions
                        .iter()
                        .map(TransitionDef::to_value)
                        .collect(),
                ),
            ),
        ])
    }

    fn from_value(value: &Value) -> Result<Self, String> {
        let [initial, states, transitions] =
            structure(value, ["initial", "states", "transitions"])?;
        Ok(Self {
            initial: text(initial, "initial")?,
            states: read_list(states, "states", StateDef::from_value)?,
            transitions: read_list(transitions, "transitions", TransitionDef::from_value)?,
        })
    }
}

impl StateDef {
    fn descriptor() -> FieldDescriptor {
        FieldDescriptor::new(
            "state",
            FieldType::Struct(vec![
                text_field("name").doc("状态名"),
                text_field("clip").doc("模型上的剪辑名"),
                FieldDescriptor::new("looping", FieldType::Bool, Value::Bool(default_looping())),
                number_field("speed", default_speed()).doc("播放速度倍数"),
            ]),
            Self::default().to_value(),
        )
    }

    fn to_value(&self) -> Value {
        record([
            ("name", text_value(&self.name)),
            ("clip", text_value(&self.clip)),
            ("looping", Value::Bool(self.looping)),
            ("speed", number_value(self.speed)),
        ])
    }

    fn from_value(value: &Value) -> Result<Self, String> {
        let [name, clip, looping, speed] = structure(value, ["name", "clip", "looping", "speed"])?;
        Ok(Self {
            name: text(name, "name")?,
            clip: text(clip, "clip")?,
            looping: looping.as_bool().ok_or("looping 必须是布尔值")?,
            speed: number(speed, "speed")?,
        })
    }
}

impl TransitionDef {
    fn descriptor() -> FieldDescriptor {
        FieldDescriptor::new(
            "transition",
            FieldType::Struct(vec![
                FieldDescriptor::new(
                    "from",
                    FieldType::Enum(vec![unit_branch("Any"), text_field("State")]),
                    branch("Any", record([])),
                )
                .doc("任意状态或指定来源状态"),
                text_field("to").doc("目标状态名"),
                FieldDescriptor::new(
                    "when",
                    FieldType::Enum(vec![
                        unit_branch("Always"),
                        condition_branch("AtLeast"),
                        condition_branch("Below"),
                    ]),
                    branch("Always", record([])),
                )
                .doc("无条件，或与参数阈值比较"),
                number_field("duration", 0.0)
                    .unit("s")
                    .doc("淡入时长；非正值使用 Animator 的设置"),
            ]),
            Self::default().to_value(),
        )
    }

    fn to_value(&self) -> Value {
        let from = self.from.as_ref().map_or_else(
            || branch("Any", record([])),
            |state| branch("State", text_value(state)),
        );
        let when = match &self.when {
            None => branch("Always", record([])),
            Some(TransitionCondition::AtLeast { parameter, value }) => {
                branch("AtLeast", condition_value(parameter, *value))
            }
            Some(TransitionCondition::Below { parameter, value }) => {
                branch("Below", condition_value(parameter, *value))
            }
        };
        record([
            ("from", from),
            ("to", text_value(&self.to)),
            ("when", when),
            ("duration", number_value(self.duration)),
        ])
    }

    fn from_value(value: &Value) -> Result<Self, String> {
        let [from, to, when, duration] = structure(value, ["from", "to", "when", "duration"])?;
        let from = match enum_parts(from, "from")? {
            ("Any", payload) => {
                structure(payload, [])?;
                None
            }
            ("State", payload) => Some(text(payload, "from.State")?),
            (variant, _) => return Err(format!("未知的 from 分支：{variant}")),
        };
        let when = match enum_parts(when, "when")? {
            ("Always", payload) => {
                structure(payload, [])?;
                None
            }
            (variant @ ("AtLeast" | "Below"), payload) => {
                let [parameter, value] = structure(payload, ["parameter", "value"])?;
                let parameter = text(parameter, "parameter")?;
                let value = number(value, "value")?;
                Some(if variant == "AtLeast" {
                    TransitionCondition::AtLeast { parameter, value }
                } else {
                    TransitionCondition::Below { parameter, value }
                })
            }
            (variant, _) => return Err(format!("未知的 when 分支：{variant}")),
        };
        Ok(Self {
            from,
            to: text(to, "to")?,
            when,
            duration: number(duration, "duration")?,
        })
    }
}

fn record<const N: usize>(fields: [(&str, Value); N]) -> Value {
    Value::Struct(
        fields
            .into_iter()
            .map(|(name, value)| (name.into(), value))
            .collect(),
    )
}

fn structure<'a, const N: usize>(
    value: &'a Value,
    names: [&str; N],
) -> Result<[&'a Value; N], String> {
    let Value::Struct(fields) = value else {
        return Err("期望结构体".into());
    };
    if fields.len() != N {
        return Err(format!("结构体字段应为 {names:?}"));
    }
    let mut values = [value; N];
    for (slot, name) in values.iter_mut().zip(names) {
        *slot = fields.get(name).ok_or_else(|| format!("缺少字段 {name}"))?;
    }
    Ok(values)
}

fn list_field(name: &'static str, element: FieldDescriptor) -> FieldDescriptor {
    FieldDescriptor::new(
        name,
        FieldType::List(Box::new(element)),
        Value::List(Vec::new()),
    )
}

fn read_list<T>(
    value: &Value,
    name: &str,
    read: fn(&Value) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    let Value::List(values) = value else {
        return Err(format!("{name} 必须是列表"));
    };
    values
        .iter()
        .enumerate()
        .map(|(index, value)| read(value).map_err(|err| format!("{name}[{index}]：{err}")))
        .collect()
}

fn branch(variant: &str, payload: Value) -> Value {
    Value::Enum {
        variant: variant.into(),
        payload: Box::new(payload),
    }
}

fn enum_parts<'a>(value: &'a Value, name: &str) -> Result<(&'a str, &'a Value), String> {
    match value {
        Value::Enum { variant, payload } => Ok((variant, payload)),
        _ => Err(format!("{name} 必须是枚举")),
    }
}

fn unit_branch(name: &'static str) -> FieldDescriptor {
    FieldDescriptor::new(name, FieldType::Struct(Vec::new()), record([]))
}

fn condition_branch(name: &'static str) -> FieldDescriptor {
    FieldDescriptor::new(
        name,
        FieldType::Struct(vec![text_field("parameter"), number_field("value", 0.0)]),
        condition_value("", 0.0),
    )
}

fn condition_value(parameter: &str, value: f32) -> Value {
    record([
        ("parameter", text_value(parameter)),
        ("value", number_value(value)),
    ])
}

fn text_field(name: &'static str) -> FieldDescriptor {
    FieldDescriptor::new(name, FieldType::String, Value::String(String::new()))
}

fn text_value(text: &str) -> Value {
    Value::String(text.into())
}

fn text(value: &Value, field: &str) -> Result<String, String> {
    value
        .as_string()
        .map(str::to_owned)
        .ok_or_else(|| format!("{field} 必须是字符串"))
}

const MIN_NUMBER: f64 = -(f32::MAX as f64);
const MAX_NUMBER: f64 = f32::MAX as f64;

fn number_field(name: &'static str, default: f32) -> FieldDescriptor {
    FieldDescriptor::new(name, FieldType::F32, number_value(default)).range(MIN_NUMBER, MAX_NUMBER)
}

fn number_value(value: f32) -> Value {
    Value::F64(f64::from(value))
}

fn number(value: &Value, field: &str) -> Result<f32, String> {
    match value {
        Value::F64(value) if (MIN_NUMBER..=MAX_NUMBER).contains(value) => Ok(*value as f32),
        _ => Err(format!("{field} 必须是 f32 可表示的有限数")),
    }
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
