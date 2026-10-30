//! 触摸点：屏幕上的手指（或触控笔）位置与相位。
//!
//! # 为什么单独一套而不是"多点鼠标"
//!
//! 鼠标只有一个指针，"按下了"是布尔量；触摸是**多个独立指针**，每个有自己的
//! 生命周期（按下 → 移动 → 抬起 / 被系统取消）。把触摸硬塞进鼠标的
//! `buttons_down` 会让"两根手指"永远只有一个状态位 —— 捏合缩放、双摇杆这类
//! 玩法就写不出来。
//!
//! 所以这里是完整的一套，同时**额外**把第一根手指映射成鼠标（见
//! [`InputState::handle_touch`]）：`ui_*` 那套按钮 / 面板不用改一个字就能响应
//! 触摸 —— 桌面引擎上"顺手点一下"比"为触摸重写一套 UI 交互"重要得多。
//!
//! # 相位语义
//!
//! | 相位 | 含义 | 何时消失 |
//! |---|---|---|
//! | [`Started`](TouchPhase::Started) | 这一帧刚按下 | 下一帧变 `Moved` 或消失 |
//! | [`Moved`](TouchPhase::Moved) | 按着并且动了 | 抬起时 |
//! | [`Ended`](TouchPhase::Ended) | 这一帧抬起了 | 帧末（[`InputState::end_frame`]） |
//! | [`Cancelled`](TouchPhase::Cancelled) | 被系统打断（来电 / 手势接管） | 帧末，与 `Ended` 同样处理 |
//!
//! `Ended` 的那一帧**仍留在表里**：`touch_ended` 是"本帧刚抬起"的唯一信号，
//! 抬手的瞬间就把它删掉的话，游戏永远收不到"手指离开"。

use xgame_math::glam::Vec2;

/// 一根手指的生命周期相位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TouchPhase {
    Started,
    Moved,
    Ended,
    Cancelled,
}

impl TouchPhase {
    /// 抬起 / 取消：帧末会从表里删掉。
    pub fn is_finished(self) -> bool {
        matches!(self, TouchPhase::Ended | TouchPhase::Cancelled)
    }

    /// 脚本 / 日志用的稳定名字（`started` / `moved` / `ended` / `cancelled`）。
    pub fn name(self) -> &'static str {
        match self {
            TouchPhase::Started => "started",
            TouchPhase::Moved => "moved",
            TouchPhase::Ended => "ended",
            TouchPhase::Cancelled => "cancelled",
        }
    }
}

/// 一个触点。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchPoint {
    pub phase: TouchPhase,
    /// 逻辑屏幕坐标（与 `mouse_pos` 同一坐标系，原点在窗口左上角）。
    pub position: Vec2,
    /// 压力 / 接触面积（0..1，设备不给时是 0）。触控笔的压感从这里进。
    pub force: f32,
}

impl TouchPoint {
    pub fn new(phase: TouchPhase, position: Vec2, force: f32) -> Self {
        Self {
            phase,
            position,
            force: if force.is_finite() {
                force.clamp(0.0, 1.0)
            } else {
                0.0
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finished_phases_are_ended_and_cancelled() {
        assert!(!TouchPhase::Started.is_finished());
        assert!(!TouchPhase::Moved.is_finished());
        assert!(TouchPhase::Ended.is_finished());
        assert!(TouchPhase::Cancelled.is_finished());
    }

    #[test]
    fn force_is_clamped_and_nan_free() {
        assert_eq!(TouchPoint::new(TouchPhase::Moved, Vec2::ZERO, 5.0).force, 1.0);
        assert_eq!(TouchPoint::new(TouchPhase::Moved, Vec2::ZERO, -1.0).force, 0.0);
        assert_eq!(
            TouchPoint::new(TouchPhase::Moved, Vec2::ZERO, f32::NAN).force,
            0.0
        );
    }
}
