//! 混合层的配置和播放状态由模拟、脚本和渲染共用。
//!
//! 模拟推进层时钟，渲染在骨骼局部空间混合姿势。层机器与基础动画共用
//! `StateMachine`，因此转移条件、参数和淡入语义可沿用同一份定义。

use std::collections::BTreeMap;

use crate::StateMachine;

/// 一层的混合模式。默认与旧端一致：加增量。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LayerMode {
    #[default]
    Additive,
    Override,
}

/// 层栈里的一层：配置 + 本层运行时状态。
#[derive(Clone, Debug, PartialEq)]
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
    /// 本层状态机的数值参数。
    pub parameters: BTreeMap<String, f32>,
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
            parameters: BTreeMap::new(),
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

/// 有序的动画层栈，空列表表示关闭。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnimationLayers {
    pub layers: Vec<AnimLayer>,
}
