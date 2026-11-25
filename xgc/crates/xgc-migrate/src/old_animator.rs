//! 旧 Animator / 状态机 → `animation.animator` + `animation.state` 组件数据。
//!
//! 旧 Animator 内嵌的机器迁为独立的 `animation.state`，让运行时和脚本能分别
//! 更新状态时间与参数，无需重写剪辑配置。机器与参数由组件反射导出为结构化值。

use std::collections::BTreeMap;

use serde::Deserialize;
use xgc_world::{
    AnimEvent, AnimLayer, AnimationLayers, AnimationState, Animator, Component, ComponentData,
    LayerMode, RootMotion, StateDef, StateMachine, TransitionCondition, TransitionDef, Value,
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
    /// 根运动：节点名 + 参与轴；旧端缺省不写，写了才迁。
    #[serde(default)]
    pub root_motion: Option<OldRootMotion>,
    /// 作者写在 Animator 上的动画事件；旧端缺省不写。
    #[serde(default)]
    pub events: Vec<OldAnimEvent>,
    /// 混合层栈；旧端缺省不写（空 = 没有层）。
    #[serde(default)]
    pub layers: Vec<OldLayer>,
}

/// 旧 `Animator.events` 的一条：从剪辑 0 起算的秒数 + 事件名。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct OldAnimEvent {
    pub time: f32,
    pub name: String,
}

/// 旧 `Animator.root_motion`：显式节点名 + 参与根运动的轴。
///
/// `axes` 缺省与旧端一致（X/Z 平移交给实体）；节点名不猜——不同 DCC 导出的
/// 资产生成的根节点层级不同，猜错会把骨架整体平移当成根运动。
#[derive(Clone, Debug, Deserialize)]
pub struct OldRootMotion {
    pub node: String,
    #[serde(default = "default_root_motion_axes")]
    pub axes: [bool; 3],
}

fn default_root_motion_axes() -> [bool; 3] {
    xgc_world::ROOT_MOTION_AXES_DEFAULT
}

impl OldRootMotion {
    /// 新组件只存 RON 字符串；解析 / 序列化都走 world 侧唯一类型。
    pub fn to_component_field(&self) -> Result<String, String> {
        RootMotion {
            node: self.node.clone(),
            axes: self.axes,
        }
        .to_ron()
    }
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

/// 旧 `Animator.layers` 里的一层。
///
/// 旧层有自己的剪辑 / 权重 / 时钟 / 可选状态机 / 骨骼遮罩；迁移只搬配置，
/// 运行时状态（当前状态、状态时间、淡入进度）由 XGC 侧从零推进。
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct OldLayer {
    pub name: String,
    pub mode: OldLayerMode,
    pub clip: String,
    pub weight: f32,
    pub speed: f32,
    pub looping: bool,
    /// 本层转移缺省淡入时长；旧端不写时退回 0.25（旧 `AnimLayer::default`）。
    pub fade_time: Option<f32>,
    pub machine: Option<OldMachine>,
    /// 骨骼遮罩（子树根名单）。
    pub mask: Vec<String>,
}

impl Default for OldLayer {
    fn default() -> Self {
        Self {
            name: String::new(),
            mode: OldLayerMode::default(),
            clip: String::new(),
            weight: 1.0,
            speed: 1.0,
            looping: true,
            fade_time: None,
            machine: None,
            mask: Vec::new(),
        }
    }
}

/// 旧层混合模式；缺省与旧端一致（Additive）。
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
pub enum OldLayerMode {
    #[default]
    Additive,
    Override,
}

impl OldLayer {
    /// 旧层 → 新层：机器翻成同一份 `StateMachine`，运行时状态从零起。
    pub fn to_anim_layer(&self) -> AnimLayer {
        AnimLayer {
            name: self.name.clone(),
            mode: match self.mode {
                OldLayerMode::Additive => LayerMode::Additive,
                OldLayerMode::Override => LayerMode::Override,
            },
            clip: self.clip.clone(),
            weight: self.weight,
            speed: self.speed,
            looping: self.looping,
            fade_time: self.fade_time.unwrap_or(0.25),
            mask: self.mask.clone(),
            machine: self.machine.as_ref().map(OldMachine::to_state_machine),
            parameters: self
                .machine
                .as_ref()
                .map(|machine| machine.parameters.clone())
                .unwrap_or_default(),
            ..AnimLayer::default()
        }
    }
}

/// 旧 Animator 的状态机（`Scenes/*.ron` 里的 `machine:` 节）。
///
/// 旧字段名与新类型差一个字母的地方（`entry` / `param`）在这里一次翻译完，
/// 运行时只认 `xgc_world::StateMachine`。
#[derive(Clone, Debug, Deserialize)]
pub struct OldMachine {
    entry: String,
    #[serde(default)]
    parameters: BTreeMap<String, f32>,
    #[serde(default)]
    states: Vec<OldState>,
    #[serde(default)]
    transitions: Vec<OldTransition>,
}

#[derive(Clone, Debug, Deserialize)]
struct OldState {
    name: String,
    clip: String,
    speed: f32,
    looping: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct OldTransition {
    from: String,
    to: String,
    when: OldWhen,
    #[serde(default)]
    duration: Option<f32>,
}

#[derive(Clone, Debug, Deserialize)]
enum OldWhen {
    AtLeast { param: String, value: f32 },
    Below { param: String, value: f32 },
}

impl OldMachine {
    pub fn to_animation_state(&self) -> AnimationState {
        AnimationState {
            machine: self.to_state_machine(),
            parameters: self.parameters.clone(),
            ..AnimationState::default()
        }
    }

    /// 旧机器 → `xgc_world::StateMachine`（基础机器与混合层的机器共用这一处）。
    fn to_state_machine(&self) -> StateMachine {
        StateMachine {
            initial: self.entry.clone(),
            states: self
                .states
                .iter()
                .map(|state| StateDef {
                    name: state.name.clone(),
                    clip: state.clip.clone(),
                    looping: state.looping,
                    speed: state.speed,
                })
                .collect(),
            transitions: self
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
        }
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
    pub state: Option<AnimationState>,
    pub additive_clip: String,
    pub additive_weight: f32,
    pub additive_speed: f32,
    pub additive_looping: bool,
    pub root_motion: String,
    /// 空层栈不生成 `animation.layers`。
    pub layers: Vec<AnimLayer>,
}

/// 生成 `animation.animator`（+ 有机器时再生成 `animation.state`）。
pub fn animator_components(
    input: AnimatorInput,
    events: &[OldAnimEvent],
) -> Result<Vec<ComponentData>, String> {
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
            ("root_motion".to_string(), Value::String(input.root_motion)),
        ],
    }];
    if !events.is_empty() {
        let animator = Animator {
            events: events
                .iter()
                .map(|event| AnimEvent::new(event.name.clone(), event.time))
                .collect(),
            ..Animator::default()
        };
        let value = animator
            .get_field("events")
            .expect("Animator.events 必须可读");
        Animator::default().set_field("events", &value)?;
        components[0].fields.push(("events".to_string(), value));
    }
    if !input.layers.is_empty() {
        let layers = AnimationLayers {
            layers: input.layers,
        }
        .layers_value();
        AnimationLayers::default()
            .set_field("layers", &layers)
            .map_err(|err| format!("animation.layers.layers：{err}"))?;
        components.push(ComponentData {
            component: AnimationLayers::ID,
            fields: vec![("layers".to_string(), layers)],
        });
    }
    if let Some(state) = input.state {
        // 复用组件写入校验，避免旧 RON 中的 NaN / inf 落盘成 JSON null。
        let mut validated = AnimationState::default();
        let mut fields = Vec::new();
        for field in AnimationState::fields() {
            let name = field.name;
            let value = state
                .get_field(name)
                .expect("AnimationState 声明的字段必须可读");
            validated
                .set_field(name, &value)
                .map_err(|err| format!("{}.{name}：{err}", AnimationState::NAME))?;
            fields.push((name.to_string(), value));
        }
        components.push(ComponentData {
            component: AnimationState::ID,
            fields,
        });
    }
    Ok(components)
}
