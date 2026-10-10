//! 编辑器的**帧调度策略**。
//!
//! # 这个模块存在的理由
//!
//! eframe / egui 默认是**事件驱动重绘**的：只有收到输入事件、或者有人显式调用
//! `Context::request_repaint()` 时才出一帧。这对"表单式"界面是对的 —— 没输入就没变化。
//!
//! 但编辑器里嵌着一个**实时 3D 视口**，它和表单不是一回事：
//!
//! ```text
//! 事件驱动下的编辑模式：
//!   鼠标动  → 出一帧（画面跟着动）
//!   鼠标停  → **不出帧** → 视口冻在上一帧
//! ```
//!
//! 后果有两个，用户都报过：
//!
//! 1. **状态栏的 FPS 只有动鼠标时才刷新** —— 因为它本来就只在出帧时被计算。
//! 2. **操作发涩、有延迟** —— 帧不是"一直在跑、随时吸收输入"，而是"来一个事件才算一帧"。
//!    从"输入抵达"到"像素上屏"中间多了「唤醒事件循环 → 排重绘 → 才开始算」这一整段；
//!    连续出帧时这一帧本来就已经在流水线里了，输入进来的那一刻就被吸收。
//!
//! # 但是也不能"永远满速空转"
//!
//! 早先这里曾经因为 Windows 反复投递同一坐标的 `WM_MOUSEMOVE` 而**意外地**一直
//! 满速出帧（见 `main.rs` 的 `drop_redundant_pointer_moves`）。那是"碰巧连续"，
//! 不是"有意连续"——它没有边界：用户把编辑器丢在后台、或者只是把鼠标放在窗口上不动，
//! 也照样 60fps 地 present，白烧 CPU、并且和窗口的移动/缩放模态循环抢消息线程。
//!
//! 所以要的是**有边界的连续**：该活的时候满速，真闲下来就停。
//!
//! # 三档策略
//!
//! | 策略 | 行为 | 用途 |
//! |---|---|---|
//! | [`FramePolicy::Always`] | 只要开着项目就一直满速出帧 | 最"跟手"，但空闲时持续吃 CPU |
//! | [`FramePolicy::Hybrid`] | 交互中 / 刚交互过的一小段时间内满速，否则空闲 | **默认**：兼顾跟手与空闲开销 |
//! | [`FramePolicy::Reactive`] | 纯事件驱动（改动前的行为，也是 eframe 默认） | 仅用于 A/B 对照 |
//!
//! 三档都能用环境变量 `XGAME_FRAME_POLICY` 切（`always` / `hybrid` / `reactive`），
//! 这样"哪一档更好"这件事可以用**同一台机器上的实测**来回答，而不是靠感觉。

use std::time::{Duration, Instant};

/// 最后一次输入之后，还保持满速出帧多久。
///
/// 为什么需要这段"宽限期"而不是松手就停：用户松手的那一刻界面往往还有事情在收尾
/// （gizmo 结束拖拽后的高亮、缩放指示器在淡出、滚动条的淡入动画……）。留一小段
/// 满速时间让它们走完，观感上不会出现"松手瞬间画面一僵"。
///
/// 220ms 是"够走完这些短动画"与"不会白白多烧太久 CPU"之间取的折中。
pub const ACTIVE_GRACE: Duration = Duration::from_millis(220);

/// 一帧的调度决策。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FramePlan {
    /// 立刻再排一帧（连续模式）。
    Now,
    /// 不主动排帧 —— 等系统把下一个事件送进来（空闲）。
    Idle,
}

/// 参与决策的输入。刻意做成一个"纯数据"结构，好让策略本身可以脱离 egui 单测。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FrameInputs {
    /// 正在 Play（物理与脚本必须按帧推进，绝不能停）。
    pub playing: bool,
    /// 指针有按键按下 —— 拖 gizmo、转视角、框选都算。
    ///
    /// 用"按着键"而不是"正在拖某个具体东西"：前者是**单一、可靠**的信号，
    /// 后者要分别问 gizmo / 相机 / 面板，很容易漏一处（而漏掉的那一处就会表现为
    /// "拖着拖着画面不跟手了"）。
    pub interacting: bool,
    /// 本帧收到了输入事件。
    pub had_input: bool,
}

impl FrameInputs {
    /// 没有任何输入、也没在播、也没在交互 —— 纯粹的空闲一帧。
    ///
    /// 只给测试用：生产代码构造 `FrameInputs` 时三项都来自真实状态（见
    /// `EditorApp::update`），"假装它是空闲的"没有意义。
    #[cfg(test)]
    pub const fn idle() -> Self {
        Self {
            playing: false,
            interacting: false,
            had_input: false,
        }
    }
}

/// 帧调度策略。
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum FramePolicy {
    /// 只要开着项目就持续满速出帧。
    Always,
    /// 交互中 / 刚交互过 → 满速；真正空闲 → 停。
    ///
    /// 默认档：3D 视口的跟手程度是编辑器的核心体验，而"真正空闲"（既没在交互、
    /// 也没有刚交互过）在正常使用中占的时间很短 —— 在这两种需求之间，
    /// Hybrid 用很小的代价同时拿到了两边。
    #[default]
    Hybrid,
    /// 纯事件驱动（eframe 默认行为）。
    Reactive,
}

impl FramePolicy {
    /// 从环境变量 `XGAME_FRAME_POLICY` 读策略，缺省 / 不认识就用 [`Self::default`]。
    ///
    /// 留这个开关是为了让"哪一档更好"能用实测回答：三档跑同一个驱动脚本、比同一组
    /// 指标。它也顺带给用户一个自选的余地（有人宁愿多花点 CPU 换更跟手）。
    pub fn from_env() -> Self {
        match std::env::var("XGAME_FRAME_POLICY").ok().as_deref() {
            Some("always") => Self::Always,
            Some("reactive") => Self::Reactive,
            Some("hybrid") => Self::Hybrid,
            _ => Self::default(),
        }
    }
}

/// 算本帧之后该怎么排下一帧。
///
/// `since_last_input` 是"距最后一次输入过了多久"。调用方负责维护这个时间戳
/// （见 `EditorApp::last_input_at`）—— 策略本身不持有时间，纯函数才好测。
pub fn plan(policy: FramePolicy, inputs: FrameInputs, since_last_input: Duration) -> FramePlan {
    let hot = inputs.playing
        || inputs.interacting
        || inputs.had_input
        || since_last_input < ACTIVE_GRACE;

    match policy {
        // 永远连续。
        FramePolicy::Always => FramePlan::Now,
        // 热的时候连续；冷下来就停。
        FramePolicy::Hybrid => {
            if hot {
                FramePlan::Now
            } else {
                FramePlan::Idle
            }
        }
        // 纯事件驱动：只有"本帧确实有事件"才再排一帧；其余交给系统。
        // 注意这里**不看** `since_last_input` —— 那正是改动前的行为，A/B 要用它。
        FramePolicy::Reactive => {
            if inputs.had_input {
                FramePlan::Now
            } else {
                FramePlan::Idle
            }
        }
    }
}

/// 帧调度的运行时状态：维护"最后一次输入"的时刻。
///
/// 单独做成一个结构而不是散在 `EditorApp` 里，是为了让"宽限期"这条逻辑有个
/// 明确的归属，也方便单测 —— 单测直接传一个假的 `Instant`，不用真的等 220ms。
pub struct FrameClock {
    last_input: Instant,
}

impl FrameClock {
    /// 从"现在"起算。
    pub fn new(now: Instant) -> Self {
        Self { last_input: now }
    }

    /// 记一次输入（本帧有事件时调用）。
    pub fn note_input(&mut self, now: Instant) {
        self.last_input = now;
    }

    /// 距最后一次输入过了多久。
    pub fn since_last_input(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.last_input)
    }

    /// 组合出本帧的调度决策。
    pub fn plan(&self, policy: FramePolicy, inputs: FrameInputs, now: Instant) -> FramePlan {
        plan(policy, inputs, self.since_last_input(now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Duration = Duration::from_secs(10); // 明显超出宽限期

    fn inputs(playing: bool, interacting: bool, had_input: bool) -> FrameInputs {
        FrameInputs {
            playing,
            interacting,
            had_input,
        }
    }

    // -- Reactive：改动前的行为，作为 A/B 的基准 --

    #[test]
    fn reactive_only_fires_on_actual_input() {
        assert_eq!(
            plan(FramePolicy::Reactive, inputs(false, false, true), NONE),
            FramePlan::Now
        );
        // 没有事件 → 停。哪怕"刚刚才输入过"也一样 —— 这正是旧行为的特征。
        assert_eq!(
            plan(FramePolicy::Reactive, inputs(false, false, false), Duration::ZERO),
            FramePlan::Idle,
            "Reactive 不该因为\"刚输入过\"就继续出帧，否则它就不是纯事件驱动了"
        );
        // 拖拽中但本帧没有新事件：Reactive 也会停（这就是发涩的来源）。
        assert_eq!(
            plan(FramePolicy::Reactive, inputs(false, true, false), NONE),
            FramePlan::Idle
        );
    }

    // -- Hybrid --

    #[test]
    fn hybrid_keeps_running_while_interacting() {
        // 按着键不放、但指针没动 → 仍要满速（否则拖拽会一顿一顿）。
        assert_eq!(
            plan(FramePolicy::Hybrid, inputs(false, true, false), NONE),
            FramePlan::Now,
            "按住指针期间必须连续出帧"
        );
    }

    #[test]
    fn hybrid_never_stops_during_play() {
        assert_eq!(
            plan(FramePolicy::Hybrid, inputs(true, false, false), NONE),
            FramePlan::Now,
            "Play 模式下物理必须按帧推进，绝不能停"
        );
    }

    #[test]
    fn hybrid_keeps_running_within_the_grace_window() {
        assert_eq!(
            plan(FramePolicy::Hybrid, inputs(false, false, false), ACTIVE_GRACE / 2),
            FramePlan::Now,
            "刚输入过的一小段时间内仍要满速，否则松手瞬间画面会僵住"
        );
    }

    #[test]
    fn hybrid_idles_once_the_grace_window_expires() {
        assert_eq!(
            plan(FramePolicy::Hybrid, inputs(false, false, false), ACTIVE_GRACE),
            FramePlan::Idle,
            "宽限期一过就该停下来，否则又变成无边界空转"
        );
    }

    // -- Always --

    #[test]
    fn always_never_idles() {
        assert_eq!(
            plan(FramePolicy::Always, FrameInputs::idle(), NONE),
            FramePlan::Now
        );
    }

    // -- FrameClock --

    #[test]
    fn frame_clock_tracks_the_grace_window() {
        let t0 = Instant::now();
        let mut clock = FrameClock::new(t0);

        // 刚启动、没有任何输入：距"上一次输入"是 0 → 还在宽限期内。
        assert_eq!(
            clock.plan(FramePolicy::Hybrid, FrameInputs::idle(), t0),
            FramePlan::Now
        );

        // 过了半个宽限期 → 还在。
        assert_eq!(
            clock.plan(FramePolicy::Hybrid, FrameInputs::idle(), t0 + ACTIVE_GRACE / 2),
            FramePlan::Now
        );

        // 超过宽限期 → 停。
        assert_eq!(
            clock.plan(FramePolicy::Hybrid, FrameInputs::idle(), t0 + ACTIVE_GRACE * 2),
            FramePlan::Idle
        );

        // 来一次输入 → 时钟重置 → 又活了。
        let t1 = t0 + ACTIVE_GRACE * 3;
        clock.note_input(t1);
        assert_eq!(
            clock.plan(FramePolicy::Hybrid, FrameInputs::idle(), t1),
            FramePlan::Now
        );
    }
}
