//! 实体动画组件（声明式播放配置）。
//!
//! 这里只放**要落盘的配置**：播哪个剪辑、速度、是否循环、是否自动播放、换剪辑
//! 淡入时长、可选的**动画状态机**（参数表 / 状态表 / 转移表）、可选的**叠加层**
//! （additive：在基础姿势上加增量）与**混合层栈**（多层 Additive/Override +
//! 骨骼遮罩）。采样时钟与蒙皮矩阵计算属于渲染层
//! （见 `xgame_render::animation`）——
//! 动画数据（glTF 里的骨骼与关键帧）本身就是渲染资产，把它拆到 core 之外
//! 可以让 core 继续对 GPU / glTF 一无所知。
//!
//! # 为什么是 `Entity` 的一等字段，而不是 `ComponentMap`
//!
//! `ComponentMap` 是 `#[serde(skip)]` 的运行时容器（装 `Box<dyn Any>`，不可比、
//! 不落盘）。而动画配置必须：进场景文件（声明式、可版本管理）、参与脏检查
//! （`Entity` 的 `PartialEq`）、被编辑器指纹看到（改了剪辑要触发重绘）——
//! 这三条都要求它像 `physics` / `scripts` 一样是一等字段。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// 动画交叉淡入的缺省时长（基础层与分层状态机同一口径）。
const DEFAULT_FADE_TIME: f32 = 0.25;

fn default_fade_time() -> f32 {
    DEFAULT_FADE_TIME
}

fn is_default_fade_time(value: &f32) -> bool {
    *value == DEFAULT_FADE_TIME
}

// ---------------------------------------------------------------------------
// 动画状态机（v1：参数阈值驱动的声明式切换）
// ---------------------------------------------------------------------------

/// 状态机里的一个状态：绑一个剪辑 + 自己的播放参数。
///
/// `speed` / `looping` **覆盖** `Animator` 上的同名全局字段 —— 机器模式下
/// 「Idle 慢速循环、Run 原速循环、Attack 播完不循环」这类差异是状态自己的事。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnimState {
    /// 状态名（转移表按名字引用；同一机器内唯一）。
    pub name: String,
    /// 要播的剪辑名（glTF `animations[].name`）。
    pub clip: String,
    pub speed: f32,
    pub looping: bool,
}

impl Default for AnimState {
    fn default() -> Self {
        Self {
            name: String::new(),
            clip: String::new(),
            speed: 1.0,
            looping: true,
        }
    }
}

/// 转移条件：盯一个浮点参数与阈值比大小（布尔语义用 0/1 + 阈值表达）。
///
/// **参数不存在（名字不在参数表里）时条件永不满足** —— 打错名字的转移静默
/// 不触发，比"当作 0.0"安全：0.0 会意外命中 `Below` 类阈值，让状态机一开机
/// 就跳走，那是个很难查的坑。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AnimCondition {
    /// 参数 ≥ 阈值。
    AtLeast { param: String, value: f32 },
    /// 参数 < 阈值。与同阈值的 `AtLeast` 互补不重叠；进出两个状态的阈值
    /// 各配各的（比如出 0.5、回 0.3），滞回区间自己留。
    Below { param: String, value: f32 },
}

impl AnimCondition {
    fn param(&self) -> &str {
        match self {
            AnimCondition::AtLeast { param, .. } | AnimCondition::Below { param, .. } => param,
        }
    }
}

/// 一条转移：`from` 状态下条件成立 → 切到 `to`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnimTransition {
    pub from: String,
    pub to: String,
    pub when: AnimCondition,
    /// 这条转移自己的**过渡时长**（秒）；`None` = 用所在机器的缺省时长：
    /// 基础机器取 [`Animator::fade_time`]，层机器取 [`AnimLayer::fade_time`]。
    ///
    /// 为什么需要逐条：一套动作里"走路→跑步"要 0.3 秒混，"受击→僵直"要 0 秒硬切
    /// （被打中的那一帧就该断掉），全局一个数不可能同时满足。`Some(0.0)` 是
    /// **硬切**，与把 `fade_time` 写 0 同义。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f32>,
}

/// 动画状态机：参数表 + 状态表 + 转移表。
///
/// 刻意做小的部分：没有一次性 trigger 参数。一个浮点参数表 + 阈值条件已经能
/// 表达人形角色最基本的 Idle/Walk/Run 移动组合；同一结构也可挂在 [`AnimLayer`]
/// 上，形成每个身体部位独立的一台机器。过渡时长可以逐条配
/// （[`AnimTransition::duration`]），没配的退回所在播放通道自己的 `fade_time`。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AnimMachine {
    /// 起始状态名。
    pub entry: String,
    /// 参数初值表。运行时脚本 / 编辑器 Inspector 直接改这张表 —— 它随场景
    /// 落盘，保存时的当前值就是下次运行的初值（Play 模式跑在场景快照上，
    /// 试玩时的改动不会污染编辑中那份文件）。
    pub parameters: BTreeMap<String, f32>,
    pub states: Vec<AnimState>,
    /// 按书写顺序求值，**第一条**满足条件的转移生效。
    pub transitions: Vec<AnimTransition>,
}

impl AnimMachine {
    /// 按名字查状态。
    pub fn state(&self, name: &str) -> Option<&AnimState> {
        self.states.iter().find(|s| s.name == name)
    }

    /// 从 `current` 出发第一条满足条件的转移 → 目标状态名。
    ///
    /// 目标状态不存在（编辑期改漏了）的转移被跳过，继续看下一条 ——
    /// 坏一条转移不该把整台状态机卡死。
    pub fn transition_for(
        &self,
        current: &str,
        parameters: &BTreeMap<String, f32>,
    ) -> Option<&str> {
        self.transition_from(current, parameters).map(|t| t.to.as_str())
    }

    /// 同 [`transition_for`](Self::transition_for)，但返回**整条转移** ——
    /// 调用方要用它自己的过渡时长（[`AnimTransition::duration`]）。
    pub fn transition_from(
        &self,
        current: &str,
        parameters: &BTreeMap<String, f32>,
    ) -> Option<&AnimTransition> {
        for t in &self.transitions {
            if t.from != current {
                continue;
            }
            let Some(&value) = parameters.get(t.when.param()) else {
                continue;
            };
            let met = match &t.when {
                AnimCondition::AtLeast {
                    value: threshold, ..
                } => value >= *threshold,
                AnimCondition::Below {
                    value: threshold, ..
                } => value < *threshold,
            };
            if met && self.state(&t.to).is_some() {
                return Some(t);
            }
        }
        None
    }

    /// 这台机器是否可用：`entry` 指得到真实状态。空壳 / 坏配置按"没有机器"处理，
    /// 退回单剪辑模式 —— 配置写坏时引擎照常跑，只是不动而已。
    pub fn is_usable(&self) -> bool {
        self.state(&self.entry).is_some()
    }
}

/// 叠加（additive）层：在基础姿势之上**加一层增量**，而不是替换它。
///
/// 数学上：`最终局部姿势 = 基础姿势 + weight × (叠加剪辑当前帧 − 叠加剪辑第 0 帧)`。
/// 第 0 帧就是这层的**参考姿势**（"零增量"帧）—— 制作叠加剪辑时把起始姿势摆成
/// 想要的静止参考，引擎减掉它，剩下的就是纯增量，叠到谁身上都是"加了个动作"。
///
/// 用途：走路时呼吸起伏、边跑边扭头、受击晃动 —— 同一份增量叠在任意基础姿势上，
/// 不必烘进每条移动剪辑（O(M+N) 取代 O(M×N) 的剪辑数量）。交叉淡入只解决
/// "换"剪辑；"叠"必须有它。
///
/// v1 边界：**全骨架、单层**（骨骼遮罩 / 多层栈由 [`AnimLayer`] 表达）；换叠加
/// 剪辑是硬切（想平滑就把 weight 脚本拉到 0、换好剪辑再拉回）。叠加层与基础层
/// 只共享 [`Animator::autoplay`] 总闸，各自按自己的速度推进 —— 基础层冻结不该连带
/// 冻住仍在播放的呼吸 / 受击增量。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnimAdditive {
    /// 叠加层剪辑名。空串 / 模型里没有 → 叠加层不生效（基础姿势原样，不报错）。
    pub clip: String,
    /// 权重 ∈ [0,1]（引擎侧钳位）：`0` = 完全不叠加，`1` = 全额增量。
    /// 脚本 `set_anim_additive_weight` 与 Inspector 都可推；随场景落盘。
    pub weight: f32,
    /// 叠加层自己的播放速度（呼吸循环可以比步频快或慢）。
    pub speed: f32,
    /// 叠加层是否循环。
    pub looping: bool,
}

impl Default for AnimAdditive {
    fn default() -> Self {
        Self {
            clip: String::new(),
            weight: 1.0,
            speed: 1.0,
            looping: true,
        }
    }
}

/// 混合层的模式。
///
/// - [`Additive`](Self::Additive)：加**增量**（当前帧 − 第 0 帧参考姿势），不替换
///   基础姿势 —— 与单层快捷方式 [`AnimAdditive`] 同语义，适合呼吸 / 弹跳 / 晃动。
/// - [`Override`](Self::Override)：向该层的姿势**整段 lerp**（覆盖）—— 配上遮罩就是
///   "上半身播放另一条剪辑、下半身继续走路"这类分区域重定向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AnimLayerMode {
    #[default]
    Additive,
    Override,
}

/// 混合层栈里的一层：模式 + 播放源 + 权重 + 独立时钟 + 骨骼遮罩。
///
/// 与 [`AnimAdditive`]（单层快捷方式）的关系：`Animator.additive` 等价于排在
/// `layers` **之前**求值的一个 Additive 层；栈内各层按数组顺序依次合到
/// "当前已合成的姿势"上（后层盖前层）。
///
/// `machine` 可选：没配时直接播 `clip`（脚本换剪辑仍是硬切，保持旧语义）；配了
/// 可用机器时，`clip` / `speed` / `looping` 被状态表覆盖，转移按本层自己的状态、
/// 参数、时钟与淡入推进。于是 locomotion 管腿、aim/fire 管上身，两台机器互不抢
/// 当前状态。`fade_time` 是本层转移未写 `duration` 时的后备值。
///
/// v1 边界：二值遮罩（权重整体缩放，不羽化边界）；所有层仍共用
/// [`Animator::autoplay`] 总闸门。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnimLayer {
    /// 层名：脚本 `set_anim_layer_weight(ctx, name, w)` 按名推权重，Inspector 也显示。
    /// 空名 / 重名时按栈内顺序取第一个命中。
    pub name: String,
    /// 求值模式（默认加增量，见 [`AnimLayerMode`]）。
    pub mode: AnimLayerMode,
    /// 这层播哪个剪辑。空串 / 模型里没有 → 这层不生效（不报错）。
    pub clip: String,
    /// 权重 ∈ [0,1]（引擎侧钳位）：`0` = 这层不参与，`1` = 全额。
    pub weight: f32,
    /// 这层自己的播放速度。
    pub speed: f32,
    /// 这层是否循环。
    pub looping: bool,
    /// 本层状态切换的缺省交叉淡入时长（秒）；逐条 `AnimTransition.duration` 优先。
    ///
    /// 缺省值不落盘：老场景里没有这个字段，读写后不该只因引擎升级就多一行。
    #[serde(
        default = "default_fade_time",
        skip_serializing_if = "is_default_fade_time"
    )]
    pub fade_time: f32,
    /// 本层自己的状态机。可用时覆盖 `clip` / `speed` / `looping`。
    ///
    /// `None` 不落盘，既保持老场景字节稳定，也让“分层状态机默认关闭”一眼可见。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<AnimMachine>,
    /// 骨骼遮罩 = **子树根名单**：节点名等于名单里任一名字、或其某个祖先等于，
    /// 则这层作用于它（`mask: ["Spine1"]` = 上半身；`["LeftUpLeg","RightUpLeg"]`
    /// = 双腿）。空名单 = 整副骨架。glTF 匿名节点匹配不到（只能靠有名字的祖先入列）。
    pub mask: Vec<String>,
}

impl Default for AnimLayer {
    fn default() -> Self {
        Self {
            name: String::new(),
            mode: AnimLayerMode::Additive,
            clip: String::new(),
            weight: 1.0,
            speed: 1.0,
            looping: true,
            fade_time: DEFAULT_FADE_TIME,
            machine: None,
            mask: Vec::new(),
        }
    }
}

impl AnimLayer {
    /// 当前生效的层状态机（坏配置退回直接剪辑模式）。
    pub fn active_machine(&self) -> Option<&AnimMachine> {
        self.machine.as_ref().filter(|m| m.is_usable())
    }

    /// 这层有没有可能推进的播放时钟（不含 `weight`；零权重时相位仍可继续走）。
    pub fn is_active(&self) -> bool {
        match self.active_machine() {
            Some(machine) => machine.states.iter().any(|state| state.speed > 0.0),
            None => !self.clip.is_empty() && self.speed > 0.0,
        }
    }
}

/// 挂在实体上的动画播放器配置。
///
/// 默认值 = 「播第一个剪辑、原速、循环、自动播放、换剪辑 0.25s 淡入」——给一个
/// 带动画的模型挂上组件后不改任何字段就应当能看到它动起来、切剪辑平滑过渡。
///
/// `#[serde(default)]` 在**结构体**级别：反序列化时缺哪个字段就从
/// [`Default`] 补哪个字段。于是旧场景文件（没有 `animator`）反序列化成
/// `None`（`Option` 缺省），手写的半截配置（比如只有 `clip`）也能补全。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Animator {
    /// 要播放的剪辑名（glTF `animations[].name`）。空串 / 找不到 = 不动画（绑定姿态）。
    pub clip: String,
    /// 播放速度倍率。`1.0` = 原速；`0` = 冻结在当前帧；负值 v1 停在第 0 帧（倒放留到 v2）。
    pub speed: f32,
    /// 是否循环。`false` 播到末尾停在最后一帧（时间被钳住，不回跳）。
    pub looping: bool,
    /// 是否自动开始播放。`false` = 静止在剪辑第 0 帧（用于「先摆好再播」的制作流程）。
    pub autoplay: bool,
    /// 换剪辑时的交叉淡入时长（秒）。`> 0` 时切剪辑不硬跳，而是从旧剪辑的当前
    /// 姿势在这么多秒内混到新剪辑；`0` = 硬切。只对**播放中**的切换生效。
    /// 机器模式下，这条是**换状态**的缺省过渡时长：转移自己写了 `duration`
    /// 就用它（见 `AnimTransition::duration`），没写才用这里。
    pub fade_time: f32,
    /// 动画状态机。`Some` 且 [`AnimMachine::is_usable`] 时进入机器模式：
    /// `clip` / `speed` / `looping` 被状态表覆盖（`autoplay` / `fade_time` 仍生效），
    /// 每帧按参数表求值转移、自动换剪辑。
    pub machine: Option<AnimMachine>,
    /// 叠加（additive）层。`Some` 时在基础姿势（单剪辑或状态机当前状态）之上
    /// **加一层增量**（见 [`AnimAdditive`]）—— 与机器模式正交：状态机换状态照常
    /// 换基础层，叠加层按自己的时钟独立播放。这是**单层快捷方式**：等价于排在
    /// [`Animator::layers`] 之前求值的一个 Additive 层，两者可以并存。
    pub additive: Option<AnimAdditive>,
    /// 混合层栈（见 [`AnimLayer`]）：每层可选 Additive / Override 模式、独立时钟与
    /// 骨骼遮罩，按数组顺序依次合到当前姿势上。求值次序：基础 → `additive` →
    /// `layers[0..]`（后层盖前层）。
    pub layers: Vec<AnimLayer>,
}

impl Default for Animator {
    fn default() -> Self {
        Self {
            clip: String::new(),
            speed: 1.0,
            looping: true,
            autoplay: true,
            fade_time: DEFAULT_FADE_TIME,
            machine: None,
            additive: None,
            layers: Vec::new(),
        }
    }
}

impl Animator {
    /// 指定剪辑、其余取默认的便捷构造。
    pub fn new(clip: impl Into<String>) -> Self {
        Self {
            clip: clip.into(),
            ..Self::default()
        }
    }

    /// 这一帧该不该推进时钟（`viewport_animating` / 帧策略据此判断）。
    ///
    /// 三个条件缺一不可：没开自动播放就没有"在播"这回事；速度 ≤ 0 时时间不前进
    /// ——负速的语义是"从第 0 帧起倒放"，而第 0 帧倒着走只会钳回 0，画面纹丝不动，
    /// 继续请求出帧就是白烧 CPU（倒放要从剪辑末尾起步才有意义，留到 v2）。
    ///
    /// 机器模式下的门控：`autoplay` 且**任一实际播放通道**可能推进。基础层、
    /// additive 与混合层栈共享 `autoplay`，但各自按自己的速度判断；这让基础姿势
    /// 冻住时，上半身状态机仍能从 Aim 转到 Fire。
    ///
    /// 不按"当前状态"判 —— 一个速度 0 的状态（冻姿势）仍要继续求值转移，
    /// 否则机器会卡死在冻结状态里；而所有状态都是 0 才是真正的"整台不播"，
    /// 那就别请求出帧了。
    pub fn is_active(&self) -> bool {
        if !self.autoplay {
            return false;
        }
        let base = self.base_is_active();
        let additive = self
            .additive
            .as_ref()
            .is_some_and(|layer| !layer.clip.is_empty() && layer.speed > 0.0);
        base || additive || self.layers.iter().any(AnimLayer::is_active)
    }

    /// 只看**基础层**有没有可能推进（不含 `autoplay` 总闸与附加层）。
    ///
    /// 编辑器帧策略要分别判断“基础 / additive / layers 哪条真的会改画面”；若只问
    /// [`is_active`](Self::is_active)，一个权重为 0 的活层会让冻结的基础剪辑也被误判
    /// 成每帧变化，视口就会空转。
    pub fn base_is_active(&self) -> bool {
        match self.active_machine() {
            Some(machine) => machine.states.iter().any(|state| state.speed > 0.0),
            None => self.speed > 0.0,
        }
    }

    /// 当前生效的状态机（配置坏掉的机器视作不存在，退回单剪辑模式）。
    pub fn active_machine(&self) -> Option<&AnimMachine> {
        self.machine.as_ref().filter(|m| m.is_usable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 默认值的语义承诺：挂上就播、原速、循环、切剪辑淡入。
    #[test]
    fn default_plays_the_first_clip_in_a_loop() {
        let a = Animator::default();
        assert!(a.autoplay, "默认应当自动播放");
        assert!(a.looping, "默认应当循环");
        assert!((a.speed - 1.0).abs() < f32::EPSILON, "默认应当原速");
        assert!(
            (a.fade_time - 0.25).abs() < f32::EPSILON,
            "默认换剪辑应有 0.25s 淡入"
        );
        assert!(a.is_active());
    }

    /// 旧场景文件没有 `animator` 字段 → `None`；半截配置由结构体默认补全。
    #[test]
    fn partial_ron_config_fills_missing_fields_from_defaults() {
        #[derive(Deserialize)]
        struct Wrap {
            #[serde(default)]
            animator: Animator,
        }
        let partial: Wrap = ron::from_str("(animator: (clip: \"Run\"))").unwrap();
        assert_eq!(partial.animator.clip, "Run");
        assert!(partial.animator.looping, "缺失的 looping 应补成默认 true");
        assert!(partial.animator.autoplay, "缺失的 autoplay 应补成默认 true");
        assert!(
            (partial.animator.speed - 1.0).abs() < f32::EPSILON,
            "缺失的 speed 应补成 1.0"
        );
        assert!(
            (partial.animator.fade_time - 0.25).abs() < f32::EPSILON,
            "缺失的 fade_time 应补成默认 0.25s"
        );

        // 显式 0 必须原样保留 —— 它是「我要硬切」的表达，不能被默认值顶掉。
        let hard: Wrap = ron::from_str("(animator: (clip: \"Run\", fade_time: 0.0))").unwrap();
        assert_eq!(hard.animator.fade_time, 0.0, "显式 fade_time: 0 是硬切语义");

        // 整个字段都缺失 → 用整套默认（clip 空 = 不动画）。
        let none: Wrap = ron::from_str("()").unwrap();
        assert_eq!(none.animator.clip, "", "完全没有 animator 字段时应是默认值");
        assert!(none.animator.looping);
    }

    /// 没开自动播放 / 速度不为正 → 不请求出帧（否则编辑器会满速空转）。
    #[test]
    fn inactive_when_paused_or_stationary() {
        let mut a = Animator::new("Walk");
        assert!(a.is_active());
        a.autoplay = false;
        assert!(!a.is_active(), "关掉自动播放就不该再请求出帧");
        a.autoplay = true;
        a.speed = 0.0;
        assert!(!a.is_active(), "速度 0 时时间不前进，不该白烧帧");
    }

    /// 负速 = 从第 0 帧起倒放 → 钳回 0、画面不动 → **不该**请求出帧。
    /// （真正的倒放需要"从剪辑末尾起步"，是 v2 的事；v1 在这里显式声明语义，
    /// 免得未来有人以为是 bug 又把满速空转加回来。）
    #[test]
    fn negative_speed_is_stationary_in_v1_and_must_not_spin_frames() {
        let mut a = Animator::new("Walk");
        a.speed = -1.0;
        assert!(!a.is_active(), "v1 负速停在第 0 帧，不该请求出帧");
    }

    // -----------------------------------------------------------------------
    // 状态机
    // --------------------------------------------------------------------

    /// 一台 Idle/Walk/Run 的最小机器（Fox 演示场景的形状）。
    fn locomotion() -> AnimMachine {
        AnimMachine {
            entry: "Idle".into(),
            parameters: BTreeMap::from([("speed".to_string(), 0.0)]),
            states: vec![
                AnimState {
                    name: "Idle".into(),
                    clip: "Survey".into(),
                    ..Default::default()
                },
                AnimState {
                    name: "Walk".into(),
                    clip: "Walk".into(),
                    ..Default::default()
                },
                AnimState {
                    name: "Run".into(),
                    clip: "Run".into(),
                    ..Default::default()
                },
            ],
            transitions: vec![
                AnimTransition {
                    from: "Idle".into(),
                    to: "Walk".into(),
                    when: AnimCondition::AtLeast {
                        param: "speed".into(),
                        value: 0.5,
                    },
                    duration: None,
                },
                AnimTransition {
                    from: "Walk".into(),
                    to: "Idle".into(),
                    when: AnimCondition::Below {
                        param: "speed".into(),
                        value: 0.3,
                    },
                    duration: None,
                },
                AnimTransition {
                    from: "Walk".into(),
                    to: "Run".into(),
                    when: AnimCondition::AtLeast {
                        param: "speed".into(),
                        value: 2.0,
                    },
                    duration: None,
                },
                AnimTransition {
                    from: "Run".into(),
                    to: "Walk".into(),
                    when: AnimCondition::Below {
                        param: "speed".into(),
                        value: 1.8,
                    },
                    duration: None,
                },
            ],
        }
    }

    fn params(speed: f32) -> BTreeMap<String, f32> {
        BTreeMap::from([("speed".to_string(), speed)])
    }

    /// 半截 RON 配置由默认补全；机器嵌在 `Option` 里（RON 必须显式 `Some`）。
    #[test]
    fn machine_ron_roundtrip_fills_defaults() {
        #[derive(Deserialize)]
        struct Wrap {
            #[serde(default)]
            animator: Animator,
        }
        let ron = r#"(animator: (
            fade_time: 0.3,
            machine: Some((
                entry: "Idle",
                states: [(name: "Idle", clip: "Survey"), (name: "Walk", clip: "Walk")],
            )),
        ))"#;
        let a: Wrap = ron::from_str(ron).unwrap();
        let m = a.animator.machine.as_ref().expect("machine 应解析出来");
        assert_eq!(m.entry, "Idle");
        assert!(m.parameters.is_empty(), "缺失 parameters 应补成空表");
        assert_eq!(m.states.len(), 2);
        let walk = m.state("Walk").expect("应能按名查状态");
        assert!(walk.looping, "缺失 looping 应补默认 true");
        assert!((walk.speed - 1.0).abs() < f32::EPSILON);
        assert!(m.transitions.is_empty());
        assert!(m.is_usable());
    }

    /// 叠加层默认语义：挂上即全额（weight 1.0）、原速、循环；Animator 默认不叠。
    #[test]
    fn additive_defaults_are_full_weight_and_off_by_default() {
        let d = AnimAdditive::default();
        assert!((d.weight - 1.0).abs() < f32::EPSILON, "默认全额叠加");
        assert!((d.speed - 1.0).abs() < f32::EPSILON, "默认原速");
        assert!(d.looping, "默认循环");
        assert!(Animator::default().additive.is_none(), "默认不叠任何东西");
    }

    /// 半截 RON：叠加层只写 clip/weight，其余补默认；与状态机**正交共存**；往返保留。
    #[test]
    fn additive_ron_fills_defaults_and_coexists_with_machine() {
        #[derive(Deserialize)]
        struct Wrap {
            #[serde(default)]
            animator: Animator,
        }
        let ron = r#"(animator: (
            clip: "Walk",
            machine: Some((entry: "Idle", states: [(name: "Idle", clip: "Survey")])),
            additive: Some((clip: "Run", weight: 0.4)),
        ))"#;
        let a: Wrap = ron::from_str(ron).unwrap();
        let add = a.animator.additive.as_ref().expect("additive 应解析出来");
        assert_eq!(add.clip, "Run");
        assert!((add.weight - 0.4).abs() < f32::EPSILON);
        assert!(
            (add.speed - 1.0).abs() < f32::EPSILON,
            "缺失 speed 补默认 1.0"
        );
        assert!(add.looping, "缺失 looping 补默认 true");
        assert!(
            a.animator.active_machine().is_some(),
            "additive 与机器模式正交：机器照样可用"
        );

        let s = ron::to_string(&a.animator).expect("应能序列化");
        let back: Animator = ron::from_str(&s).expect("应能反序列化");
        assert_eq!(
            back.additive, a.animator.additive,
            "additive 应活过 RON 往返"
        );
    }

    /// 层栈默认语义：Additive 模式、全额权重、空遮罩（= 整副骨架）、Animator 默认空栈。
    #[test]
    fn layer_defaults_are_additive_full_weight_full_skeleton() {
        let d = AnimLayer::default();
        assert_eq!(d.mode, AnimLayerMode::Additive, "默认加增量而不是覆盖");
        assert!((d.weight - 1.0).abs() < f32::EPSILON, "默认全额");
        assert!((d.speed - 1.0).abs() < f32::EPSILON, "默认原速");
        assert!(d.looping, "默认循环");
        assert!((d.fade_time - 0.25).abs() < f32::EPSILON, "层转移默认淡入 0.25s");
        assert!(d.machine.is_none(), "默认不启用层状态机");
        assert!(d.mask.is_empty(), "默认整副骨架");
        assert!(Animator::default().layers.is_empty(), "默认空栈");
    }

    /// 半截 RON：层栈只写必需字段，其余补默认；Override 模式与遮罩按名解析；
    /// 与机器、单层 additive 三者**正交共存**；往返保留。
    #[test]
    fn layers_ron_parses_mode_and_mask_alongside_machine_and_additive() {
        #[derive(Deserialize)]
        struct Wrap {
            #[serde(default)]
            animator: Animator,
        }
        let ron = r#"(animator: (
            machine: Some((entry: "Idle", states: [(name: "Idle", clip: "Survey")])),
            additive: Some((clip: "Run", weight: 0.3)),
            layers: [
                (name: "upper", mode: Override, clip: "Survey", weight: 0.6,
                 mask: ["Spine1", "Neck"]),
                (name: "bounce"),   // 半截配置：Additive、全额、整副骨架
            ],
        ))"#;
        let a: Wrap = ron::from_str(ron).unwrap();
        assert!(a.animator.active_machine().is_some(), "机器照常可用");
        assert!(a.animator.additive.is_some(), "单层快捷方式照常可用");
        assert_eq!(a.animator.layers.len(), 2);

        let upper = &a.animator.layers[0];
        assert_eq!(upper.name, "upper");
        assert_eq!(
            upper.mode,
            AnimLayerMode::Override,
            "RON 裸标识符解析成枚举变体"
        );
        assert!((upper.weight - 0.6).abs() < f32::EPSILON);
        assert_eq!(upper.mask, vec!["Spine1".to_string(), "Neck".to_string()]);

        let bounce = &a.animator.layers[1];
        assert_eq!(bounce.mode, AnimLayerMode::Additive, "缺失 mode 补默认");
        assert!(
            (bounce.weight - 1.0).abs() < f32::EPSILON,
            "缺失 weight 补默认 1.0"
        );
        assert!(bounce.mask.is_empty(), "缺失 mask 补空名单（整副骨架）");

        let default_layer = ron::to_string(&AnimLayer::default()).expect("默认层应能序列化");
        assert!(
            !default_layer.contains("fade_time") && !default_layer.contains("machine"),
            "层的缺省新字段不该制造落盘噪音：{default_layer}"
        );

        let saved = ron::to_string(&a.animator).expect("应能序列化");
        let back: Animator = ron::from_str(&saved).expect("应能反序列化");
        assert_eq!(back.layers, a.animator.layers, "层栈应活过 RON 往返");
    }

    /// 分层状态机的配置不能在「读一遍再保存」时被静默吃掉。
    ///
    /// 这条回归刻意只从 RON 文本观察结果，不直接引用尚未加入结构体的新字段：
    /// 旧实现会把 `machine` / `fade_time` 当未知字段忽略，因此测试能在旧代码上
    /// **编译并红灯**，证明真实的数据丢失，而不是拿编译错误冒充回归证据。
    #[test]
    fn layered_machine_config_survives_ron_roundtrip() {
        let text = r#"(
            name: "upper",
            mode: Override,
            clip: "AimIdle",
            fade_time: 0.4,
            machine: Some((
                entry: "Idle",
                parameters: {"aim": 0.0},
                states: [
                    (name: "Idle", clip: "AimIdle"),
                    (name: "Fire", clip: "AimFire", looping: false),
                ],
                transitions: [(
                    from: "Idle",
                    to: "Fire",
                    when: AtLeast(param: "aim", value: 0.5),
                    duration: Some(0.1),
                )],
            )),
            mask: ["Spine"],
        )"#;
        let layer: AnimLayer = ron::from_str(text).expect("分层机器配置应能读取");
        let saved = ron::to_string(&layer).expect("分层机器配置应能保存");
        assert!(
            saved.contains("machine"),
            "保存后不能丢掉层状态机，实际：{saved}"
        );
        assert!(
            saved.contains("fade_time"),
            "保存后不能丢掉层自己的淡入时长，实际：{saved}"
        );
        assert!(saved.contains("AimFire"), "状态剪辑也必须活过往返：{saved}");

        let back: AnimLayer = ron::from_str(&saved).expect("保存结果应能再读");
        assert!((back.fade_time - 0.4).abs() < f32::EPSILON);
        let machine = back.active_machine().expect("层机器应保持可用");
        assert_eq!(machine.entry, "Idle");
        assert_eq!(machine.parameters.get("aim"), Some(&0.0));
    }

    /// 逐条过渡时长：缺省时**不落盘**（老场景逐字节不变），写了就活过往返。
    ///
    /// `duration: None` 走 `skip_serializing_if`，所以旧文件与"没写这条"的
    /// 新文件序列化结果一致 —— 这是"新增能力默认关 = 逐字节一致"的落点。
    #[test]
    fn transition_duration_is_optional_and_round_trips() {
        #[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
        struct Wrap {
            transitions: Vec<AnimTransition>,
        }
        // 老写法（没有 duration 字段）
        let old: Wrap = ron::from_str(
            r#"(transitions: [(from: "Idle", to: "Walk", when: AtLeast(param: "speed", value: 0.5))])"#,
        )
        .unwrap();
        assert_eq!(old.transitions[0].duration, None, "缺省 = 用全局 fade_time");
        let text = ron::to_string(&old).unwrap();
        assert!(
            !text.contains("duration"),
            "None 不该落盘，实际: {text}"
        );

        // 新写法
        let new: Wrap = ron::from_str(
            r#"(transitions: [(from: "Idle", to: "Walk",
                 when: AtLeast(param: "speed", value: 0.5), duration: Some(0.0))])"#,
        )
        .unwrap();
        assert_eq!(new.transitions[0].duration, Some(0.0), "0 = 硬切");
        let back: Wrap = ron::from_str(&ron::to_string(&new).unwrap()).unwrap();
        assert_eq!(back, new, "显式时长应活过 RON 往返");
    }

    /// 阈值配对：升速跨 0.5 进 Walk、跨 2.0 进 Run；降速低于 0.3 退回 Idle、
    /// 低于 1.8 退回 Walk。滞回区间（0.3~0.5、1.8~2.0）内不动。
    /// `AtLeast` 是 `≥`、`Below` 是严格 `<` —— 同一阈值的两条**互补不重叠**。
    #[test]
    fn transitions_fire_on_thresholds_with_hysteresis() {
        let m = locomotion();
        assert_eq!(m.transition_for("Idle", &params(0.4)), None, "阈值下不触发");
        assert_eq!(
            m.transition_for("Idle", &params(0.5)),
            Some("Walk"),
            "≥ 阈值触发"
        );
        assert_eq!(
            m.transition_for("Walk", &params(0.4)),
            None,
            "滞回区间保持 Walk"
        );
        assert_eq!(
            m.transition_for("Walk", &params(0.3)),
            None,
            "Below 是严格小于：恰好 0.3 不退回"
        );
        assert_eq!(
            m.transition_for("Walk", &params(0.29)),
            Some("Idle"),
            "低于阈值才退回"
        );
        assert_eq!(m.transition_for("Walk", &params(2.0)), Some("Run"));
        assert_eq!(
            m.transition_for("Run", &params(1.9)),
            None,
            "滞回区间保持 Run"
        );
        assert_eq!(m.transition_for("Run", &params(1.79)), Some("Walk"));
        assert_eq!(
            m.transition_for("Idle", &params(3.0)),
            Some("Walk"),
            "Idle 出发只走一条边：即使 3.0 也先到 Walk，不链跳到 Run"
        );
    }

    /// 打错名字的参数 → 条件**永不满足**（不当成 0.0，否则 Below 会误触发）。
    #[test]
    fn condition_on_missing_parameter_never_matches() {
        let m = locomotion();
        let empty = BTreeMap::new();
        assert_eq!(m.transition_for("Idle", &empty), None, "参数缺失时不该动");
        // Below(0.3) 若把缺失当 0.0 就会命中 —— 这正是要防的误触发。
        let typo = BTreeMap::from([("Speed".to_string(), 9.9)]);
        assert_eq!(m.transition_for("Walk", &typo), None, "大小写写错也不该动");
    }

    /// 目标状态不存在的转移被跳过（继续看下一条），不卡死状态机。
    #[test]
    fn transition_to_missing_state_is_skipped() {
        let mut m = locomotion();
        m.transitions.insert(
            0,
            AnimTransition {
                from: "Idle".into(),
                to: "Ghost".into(),
                when: AnimCondition::AtLeast {
                    param: "speed".into(),
                    value: 0.5,
                },
                duration: None,
            },
        );
        assert_eq!(
            m.transition_for("Idle", &params(9.0)),
            Some("Walk"),
            "坏转移应被跳过，落到后面那条好转移"
        );
    }

    /// entry 指不到状态 = 机器不可用 → 按没配机器处理（`active_machine` 返回 None、
    /// `is_active` 退回单剪辑语义），配置写坏时引擎照常跑。
    #[test]
    fn unusable_machine_falls_back_to_single_clip() {
        let mut a = Animator::new("Run");
        a.machine = Some(AnimMachine {
            entry: "Ghost".into(),
            states: vec![AnimState {
                name: "Idle".into(),
                clip: "Survey".into(),
                ..Default::default()
            }],
            ..Default::default()
        });
        assert!(a.active_machine().is_none(), "坏机器应视作不存在");
        assert!(a.is_active(), "单剪辑语义原样保留");

        // 退回的语义必须真的是单剪辑那套：速度 0 → 不请求出帧。
        a.speed = 0.0;
        assert!(!a.is_active(), "坏机器时 is_active 按单剪辑的 speed 判");
    }

    /// 机器模式的出帧门控只看状态表：clip/speed/looping 由状态覆盖，
    /// 单剪辑字段（`a.speed`）不再参与判定。
    #[test]
    fn machine_mode_overrides_single_clip_fields() {
        let mut a = Animator::new("ignored");
        a.speed = 7.0;
        a.machine = Some(locomotion());
        let m = a.active_machine().expect("好机器应生效");
        assert_eq!(
            m.state(&m.entry).expect("entry 应查得到").clip,
            "Survey",
            "entry 状态的剪辑才是入口"
        );
        assert!(a.is_active());

        // 把单剪辑速度清零也无济于事 —— 机器模式下它已被状态表覆盖。
        a.speed = 0.0;
        assert!(a.is_active(), "单剪辑 speed 不该再影响机器模式的门控");

        // entry 状态冻住（速度 0）但还有别的活状态 → 仍要出帧，
        // 否则转移求值停摆、机器卡死在冻结状态里。
        a.machine.as_mut().unwrap().states[0].speed = 0.0;
        assert!(a.is_active(), "只要还有速度为正的状态就该请求出帧");

        // 所有状态都冻住 = 整台不播 → 不请求出帧。
        for s in a.machine.as_mut().unwrap().states.iter_mut() {
            s.speed = 0.0;
        }
        assert!(!a.is_active(), "全员冻结时不该空转出帧");
    }

    /// 往返序列化：机器配置（含参数表、条件）写进 RON 再读回来语义不变。
    #[test]
    fn machine_survives_ron_roundtrip() {
        let a = Animator {
            fade_time: 0.3,
            machine: Some(locomotion()),
            ..Animator::default()
        };
        let s = ron::to_string(&a).expect("应能序列化");
        let back: Animator = ron::from_str(&s).expect("应能反序列化");
        assert_eq!(back, a, "往返后配置必须逐字段一致");
        let m = back.active_machine().expect("机器仍在");
        assert_eq!(m.transition_for("Idle", &params(0.5)), Some("Walk"));
    }
}
