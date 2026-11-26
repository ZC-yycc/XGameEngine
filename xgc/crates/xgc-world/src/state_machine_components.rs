//! 动画状态机组件：机器定义、当前状态、状态时间和数值参数。

use std::collections::BTreeMap;

use xgc_reflect::{
    Component, ComponentId, FieldDescriptor, FieldType, Record, Value, ValueProjection,
    ValueWriter, Variant,
};

#[path = "layer_reflection.rs"]
mod reflected_layers;

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
            parameters_field(),
        ]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        self.field_projection(field)
            .map(ValueProjection::projected_value)
    }

    fn hash_field_into(&self, field: &str, hash: &mut u64) {
        if let Some(value) = self.field_projection(field) {
            value.hash_projected(hash);
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        match field {
            "machine" => self.machine = StateMachine::from_value(value)?,
            "state" => self.state = text(value, "state")?,
            "time" => self.time = number(value, "time")?,
            "parameters" => self.parameters = read_parameters(value)?,
            _ => return Err(format!("animation.state 没有字段 {field}")),
        }
        Ok(())
    }
}

impl AnimationState {
    fn field_projection(&self, field: &str) -> Option<&dyn ValueProjection> {
        match field {
            "machine" => Some(&self.machine),
            "state" => Some(&self.state),
            "time" => Some(&self.time),
            "parameters" => Some(&self.parameters),
            _ => None,
        }
    }
}

/// 基础状态机与混合层共用的机器定义。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StateMachine {
    pub initial: String,
    pub states: Vec<StateDef>,
    pub transitions: Vec<TransitionDef>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StateDef {
    pub name: String,
    pub clip: String,
    pub looping: bool,
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

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TransitionDef {
    pub from: Option<String>,
    pub to: String,
    /// 触发条件；`None` = 无条件。
    pub when: Option<TransitionCondition>,
    /// 过渡时长（秒）；`0` = 用 `Animator.fade_time` 的缺省值。
    pub duration: f32,
}

#[derive(Clone, Debug, PartialEq)]
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
            Self::default().projected_value(),
        )
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

impl ValueProjection for StateMachine {
    fn project(&self, writer: &mut ValueWriter<'_>) {
        writer.record([
            ("initial", &self.initial),
            ("states", &self.states.as_slice()),
            ("transitions", &self.transitions.as_slice()),
        ]);
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
            Self::default().projected_value(),
        )
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

impl ValueProjection for StateDef {
    fn project(&self, writer: &mut ValueWriter<'_>) {
        writer.record([
            ("name", &self.name),
            ("clip", &self.clip),
            ("looping", &self.looping),
            ("speed", &self.speed),
        ]);
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
                    unit_value("Any"),
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
                    unit_value("Always"),
                )
                .doc("无条件，或与参数阈值比较"),
                number_field("duration", 0.0)
                    .unit("s")
                    .doc("淡入时长；非正值使用 Animator 的设置"),
            ]),
            Self::default().projected_value(),
        )
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

impl ValueProjection for TransitionDef {
    fn project(&self, writer: &mut ValueWriter<'_>) {
        let empty = Record([]);
        let from = match &self.from {
            Some(state) => Variant("State", state),
            None => Variant("Any", &empty),
        };
        let payload;
        let when = match &self.when {
            None => Variant("Always", &empty),
            Some(condition) => {
                let (variant, parameter, value) = match condition {
                    TransitionCondition::AtLeast { parameter, value } => {
                        ("AtLeast", parameter, value)
                    }
                    TransitionCondition::Below { parameter, value } => ("Below", parameter, value),
                };
                payload = condition_payload(parameter, value);
                Variant(variant, &payload)
            }
        };
        writer.record([
            ("from", &from),
            ("to", &self.to),
            ("when", &when),
            ("duration", &self.duration),
        ]);
    }
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

fn unit_value(variant: &str) -> Value {
    Variant(variant, &Record([])).projected_value()
}

fn enum_parts<'a>(value: &'a Value, name: &str) -> Result<(&'a str, &'a Value), String> {
    match value {
        Value::Enum { variant, payload } => Ok((variant, payload)),
        _ => Err(format!("{name} 必须是枚举")),
    }
}

fn unit_branch(name: &'static str) -> FieldDescriptor {
    FieldDescriptor::new(
        name,
        FieldType::Struct(Vec::new()),
        Record([]).projected_value(),
    )
}

fn condition_branch(name: &'static str) -> FieldDescriptor {
    FieldDescriptor::new(
        name,
        FieldType::Struct(vec![text_field("parameter"), number_field("value", 0.0)]),
        condition_payload(&"", &0.0).projected_value(),
    )
}

fn condition_payload<'a>(parameter: &'a dyn ValueProjection, value: &'a f32) -> Record<'a, 2> {
    Record([("parameter", parameter), ("value", value)])
}

fn text_field(name: &'static str) -> FieldDescriptor {
    FieldDescriptor::new(name, FieldType::String, Value::String(String::new()))
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
    FieldDescriptor::new(name, FieldType::F32, default.projected_value())
        .range(MIN_NUMBER, MAX_NUMBER)
}

fn number(value: &Value, field: &str) -> Result<f32, String> {
    match value {
        Value::F64(value) if (MIN_NUMBER..=MAX_NUMBER).contains(value) => Ok(*value as f32),
        _ => Err(format!("{field} 必须是 f32 可表示的有限数")),
    }
}

fn parameters_field() -> FieldDescriptor {
    FieldDescriptor::new(
        "parameters",
        FieldType::Map(Box::new(number_field("parameter", 0.0))),
        Value::Map(BTreeMap::new()),
    )
    .doc("按名字保存的状态机参数；条件中未设定的参数按零处理")
}

fn read_parameters(value: &Value) -> Result<BTreeMap<String, f32>, String> {
    let Value::Map(entries) = value else {
        return Err("parameters 必须是字符串键映射".into());
    };
    entries
        .iter()
        .map(|(key, value)| number(value, key).map(|value| (key.clone(), value)))
        .collect()
}

fn default_looping() -> bool {
    true
}

fn default_speed() -> f32 {
    1.0
}
