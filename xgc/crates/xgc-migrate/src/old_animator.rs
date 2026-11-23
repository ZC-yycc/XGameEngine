//! 旧 Animator / 状态机 → `animation.animator` + `animation.state` 组件数据。
//!
//! 旧端的动画状态机挂在 Animator 里（`machine` 是内嵌结构），XGC 把它拆成
//! 独立组件 `animation.state`：机器 RON + 当前状态 + 参数 RON。拆开是因为
//! 运行时要独立推进状态时间、脚本要直接改参数，没必要每次连剪辑一起重写。
//! 转换就发生在这一层——磁盘格式一次性换成新形状，不做运行期双读。

use std::collections::BTreeMap;

use serde::Deserialize;
use xgc_world::{
    AnimationState, Animator, Component, ComponentData, StateDef, StateMachine,
    TransitionCondition, TransitionDef, Value,
};

/// 旧 Animator 本体（`components.entries[].data: Animator(...)`）。
///
/// 这里直接实现 `Deserialize` 而不是先转 `ron::Value`：旧机器里的 `when` 是
/// enum struct variant，`ron::Value` 没有变体标签，经它中转会丢成 map。
#[derive(Debug, Deserialize)]
pub struct OldAnimator {
    pub clip: String,
    pub speed: f32,
    pub looping: bool,
    pub autoplay: bool,
    pub fade_time: f32,
    #[serde(default)]
    pub machine: Option<OldMachine>,
    #[serde(default)]
    pub additive: Option<OldAdditive>,
}

/// 旧 `Animator.additive`（单层叠加快捷方式）：增量 = 剪辑当前帧 − 第 0 帧。
///
/// 只搬配置；`additive_t` / 权重脚本写值都是运行时状态，不落盘。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct OldAdditive {
    pub clip: String,
    pub weight: f32,
    pub speed: f32,
    pub looping: bool,
}

/// 旧 Animator 的状态机（`Scenes/*.ron` 里的 `machine:` 节）。
///
/// 旧字段名与新类型差一个字母的地方（`entry` / `param`）在这里一次翻译完，
/// 运行时只认 `xgc_world::StateMachine`。
#[derive(Debug, Deserialize)]
pub struct OldMachine {
    entry: String,
    #[serde(default)]
    parameters: BTreeMap<String, f32>,
    #[serde(default)]
    states: Vec<OldState>,
    #[serde(default)]
    transitions: Vec<OldTransition>,
}

#[derive(Debug, Deserialize)]
struct OldState {
    name: String,
    clip: String,
    speed: f32,
    looping: bool,
}

#[derive(Debug, Deserialize)]
struct OldTransition {
    from: String,
    to: String,
    when: OldWhen,
    #[serde(default)]
    duration: Option<f32>,
}

#[derive(Debug, Deserialize)]
enum OldWhen {
    AtLeast { param: String, value: f32 },
    Below { param: String, value: f32 },
}

/// 迁移后的机器：机器本身与初始参数各是一份 RON 字符串（与 `animation.state`
/// 组件的两个字段同形）。
pub struct MigratedMachine {
    pub machine: String,
    pub parameters: String,
}

impl MigratedMachine {
    pub fn new(old: &OldMachine) -> Result<Self, String> {
        let machine = StateMachine {
            initial: old.entry.clone(),
            states: old
                .states
                .iter()
                .map(|state| StateDef {
                    name: state.name.clone(),
                    clip: state.clip.clone(),
                    looping: state.looping,
                    speed: state.speed,
                })
                .collect(),
            transitions: old
                .transitions
                .iter()
                .map(|transition| TransitionDef {
                    from: Some(transition.from.clone()),
                    to: transition.to.clone(),
                    when: Some(match &transition.when {
                        OldWhen::AtLeast { param, value } => TransitionCondition::AtLeast {
                            parameter: param.clone(),
                            value: *value,
                        },
                        OldWhen::Below { param, value } => TransitionCondition::Below {
                            parameter: param.clone(),
                            value: *value,
                        },
                    }),
                    duration: transition.duration.unwrap_or(0.0),
                })
                .collect(),
        };
        let machine =
            ron::ser::to_string(&machine).map_err(|err| format!("状态机序列化失败：{err}"))?;
        let parameters = ron::ser::to_string(&old.parameters)
            .map_err(|err| format!("状态机参数序列化失败：{err}"))?;
        Ok(Self {
            machine,
            parameters,
        })
    }
}

pub struct AnimatorInput {
    pub clip: String,
    pub speed: f32,
    pub looping: bool,
    pub autoplay: bool,
    pub fade_time: f32,
    pub position: [f64; 3],
    pub rotation: [f64; 4],
    pub scale: [f64; 3],
    pub machine: Option<MigratedMachine>,
    pub additive_clip: String,
    pub additive_weight: f32,
    pub additive_speed: f32,
    pub additive_looping: bool,
}

/// 生成 `animation.animator`（+ 有机器时再生成 `animation.state`）。
pub fn animator_components(input: AnimatorInput) -> Vec<ComponentData> {
    let mut components = vec![ComponentData {
        component: Animator::ID,
        fields: vec![
            ("clip".to_string(), Value::String(input.clip)),
            ("speed".to_string(), Value::F64(f64::from(input.speed))),
            ("looping".to_string(), Value::Bool(input.looping)),
            ("autoplay".to_string(), Value::Bool(input.autoplay)),
            (
                "fade_time".to_string(),
                Value::F64(f64::from(input.fade_time)),
            ),
            ("base_position".to_string(), Value::Vec3(input.position)),
            ("base_rotation".to_string(), Value::Vec4(input.rotation)),
            ("base_scale".to_string(), Value::Vec3(input.scale)),
            (
                "additive_clip".to_string(),
                Value::String(input.additive_clip),
            ),
            (
                "additive_weight".to_string(),
                Value::F64(f64::from(input.additive_weight)),
            ),
            (
                "additive_speed".to_string(),
                Value::F64(f64::from(input.additive_speed)),
            ),
            (
                "additive_looping".to_string(),
                Value::Bool(input.additive_looping),
            ),
        ],
    }];
    if let Some(machine) = input.machine {
        components.push(ComponentData {
            component: AnimationState::ID,
            fields: vec![
                ("machine".to_string(), Value::String(machine.machine)),
                ("state".to_string(), Value::String(String::new())),
                ("time".to_string(), Value::F64(0.0)),
                ("parameters".to_string(), Value::String(machine.parameters)),
            ],
        });
    }
    components
}
