//! 动画混合层组件：`animation.layers` 的 RON 快照。
//!
//! 旧端把层栈挂在 `Animator.layers` 里：每层有自己的剪辑 / 权重 / 时钟 /
//! 可选状态机 / 骨骼遮罩，脚本按层名写权重与参数。XGC 把它落成一个组件字符串：
//! 配置与运行时状态（当前状态、状态时间、淡入进度、参数）都在同一份 RON 里，
//! 由 `AnimationLayerSystem` 每帧推进、脚本按层名读改写——与 `animation.state`
//! 把机器状态落在组件字符串是同一套做法，不另立第二份表示。
//!
//! 渲染侧只消费这份数据（在模型节点局部空间做 Override / Additive × 遮罩），
//! 模拟侧只负责时钟与状态机推进；两边都不再改实体根 `Transform`。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::state_machine_components::{parse_parameters, StateMachine};
use xgc_reflect::{Component, ComponentId, FieldDescriptor, FieldType, Value};

/// 一层的混合模式。默认与旧端一致：加增量。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Deserialize, Serialize)]
pub enum LayerMode {
    #[default]
    Additive,
    Override,
}

/// 层栈里的一层：配置 + 本层运行时状态。
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct AnimLayer {
    /// 层名：脚本 `set_anim_layer_weight` / `set_anim_layer_param` 按它查找。
    pub name: String,
    /// 混合模式。
    pub mode: LayerMode,
    /// 本层播哪个剪辑（配了可用机器时由状态表覆盖）。
    pub clip: String,
    /// 权重 ∈ [0,1]（引擎侧钳位）；0 = 不参与。
    pub weight: f32,
    /// 本层自己的播放速度。
    pub speed: f32,
    /// 本层是否循环。
    pub looping: bool,
    /// 本层状态切换未写 `duration` 时的兜底淡入时长（秒）。
    pub fade_time: f32,
    /// 骨骼遮罩 = **子树根名单**：节点名等于名单里任一名字、或其某个祖先等于，
    /// 则这层作用于它；空名单 = 整副骨架（旧端同一口径，匿名节点匹配不到）。
    pub mask: Vec<String>,
    /// 本层自己的状态机；可用时覆盖 `clip` / `speed` / `looping`。
    pub machine: Option<StateMachine>,

    /// 当前状态名（空 = 尚未初始化，取机器 entry）。
    pub state: String,
    /// 当前状态已持续时间（秒）。
    pub time: f32,
    /// 本层状态机的数值参数 RON map<String, f32>。
    pub parameters: String,
    /// 淡出那半边：来自哪个剪辑 / 它自己的时钟 / 速度 / 循环方式。
    pub fade_from: String,
    pub fade_from_time: f32,
    pub fade_from_speed: f32,
    pub fade_from_looping: bool,
    pub fade_elapsed: f32,
    pub fade_duration: f32,
}

impl Default for AnimLayer {
    fn default() -> Self {
        Self {
            name: String::new(),
            mode: LayerMode::Additive,
            clip: String::new(),
            weight: 1.0,
            speed: 1.0,
            looping: true,
            fade_time: 0.25,
            mask: Vec::new(),
            machine: None,
            state: String::new(),
            time: 0.0,
            parameters: "{}".to_string(),
            fade_from: String::new(),
            fade_from_time: 0.0,
            fade_from_speed: 1.0,
            fade_from_looping: true,
            fade_elapsed: 0.0,
            fade_duration: 0.0,
        }
    }
}

impl AnimLayer {
    /// 可用机器（坏配置退回直接剪辑模式）。
    pub fn usable_machine(&self) -> Option<&StateMachine> {
        self.machine.as_ref().filter(|machine| machine.is_usable())
    }

    /// 本帧生效的剪辑名：机器可用时取当前状态的定义，否则用 `clip`。
    pub fn effective_clip(&self) -> &str {
        self.usable_machine()
            .and_then(|machine| machine.state_def(&self.state))
            .map(|state| state.clip.as_str())
            .unwrap_or(self.clip.as_str())
    }

    /// 本帧生效的循环方式（与 `effective_clip` 同源）。
    pub fn effective_looping(&self) -> bool {
        self.usable_machine()
            .and_then(|machine| machine.state_def(&self.state))
            .map(|state| state.looping)
            .unwrap_or(self.looping)
    }

    /// 本帧生效的播放速度（模拟侧推进时钟用）。
    pub fn effective_speed(&self) -> f32 {
        self.usable_machine()
            .and_then(|machine| machine.state_def(&self.state))
            .map(|state| state.speed)
            .unwrap_or(self.speed)
    }

    /// 淡入是否仍在进行；是则返回（旧剪辑、旧时间、旧循环方式、alpha）。
    pub fn fade_sample(&self) -> Option<(&str, f32, bool, f32)> {
        if self.fade_from.is_empty() || self.fade_duration <= 0.0 {
            return None;
        }
        let alpha = (self.fade_elapsed / self.fade_duration).clamp(0.0, 1.0);
        Some((
            self.fade_from.as_str(),
            self.fade_from_time,
            self.fade_from_looping,
            alpha,
        ))
    }

    /// 清掉淡入残留（硬切或淡入完成）。
    pub fn clear_fade(&mut self) {
        self.fade_from.clear();
        self.fade_from_time = 0.0;
        self.fade_from_speed = 1.0;
        self.fade_from_looping = true;
        self.fade_elapsed = 0.0;
        self.fade_duration = 0.0;
    }
}

/// 组件：RON 层栈快照；空 = 不启用。
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
        "动画混合层：RON 层栈（剪辑 / 权重 / 遮罩 / 分层状态机），在基础姿势与 additive 之后按遮罩混合"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new("layers", FieldType::String, Value::String(String::new()))
                .doc("RON 层栈；空 = 不启用"),
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

/// 解析组件的 RON 层栈（坏数据返回错误，调用方静默跳过）。
pub fn parse_layers(text: &str) -> Result<Vec<AnimLayer>, String> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    ron::from_str(text).map_err(|err| format!("混合层解析失败：{err}"))
}

/// 序列化层栈回组件字符串（确定性：结构体字段序 + 无 HashMap 迭代）。
pub fn layers_to_ron(layers: &[AnimLayer]) -> String {
    ron::ser::to_string(layers).unwrap_or_else(|_| "[]".to_string())
}

/// 层栈里按名找第一层（脚本 API 的重名口径：栈内顺序取第一个）。
pub fn find_layer<'a>(layers: &'a mut [AnimLayer], name: &str) -> Option<&'a mut AnimLayer> {
    layers.iter_mut().find(|layer| layer.name == name)
}

/// 层参数 RON 的读写（脚本 `set_anim_layer_param` 用）。
pub fn layer_parameters(layer: &AnimLayer) -> BTreeMap<String, f32> {
    parse_parameters(&layer.parameters)
}

pub fn set_layer_parameter(layer: &mut AnimLayer, name: &str, value: f32) {
    let mut parameters = layer_parameters(layer);
    parameters.insert(name.to_string(), value);
    layer.parameters = ron::ser::to_string(&parameters).unwrap_or_else(|_| "{}".to_string());
}
