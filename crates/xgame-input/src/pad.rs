//! 手柄的**语义**按键与轴，以及单个手柄的状态。
//!
//! # 为什么用语义名而不是物理键名
//!
//! 手柄正面四个键在 Xbox 上是 `A/B/X/Y`、在 PlayStation 上是 `✕/○/□/△`、
//! 在 Switch 上是 `B/A/Y/X`（物理位置与字母全都不一样）。按物理位置命名
//! （`South` = 最下面那个）是唯一跨平台稳定的叫法，各家手柄驱动本身也是这么
//! 上报的（SDL 的 `A/B/X/Y` 就映射到"下/右/左/上"）。所以：
//!
//! | 本引擎 | Xbox | PlayStation | 位置 |
//! |---|---|---|---|
//! | `South` | A | ✕ | 下 |
//! | `East` | B | ○ | 右 |
//! | `West` | X | □ | 左 |
//! | `North` | Y | △ | 上 |
//!
//! 作者在 `input.ron` 里写的是语义名；接哪种手柄由[后端](crate::gamepad_hub)
//! 负责翻译。**别在游戏逻辑里按字母思考** —— 那个映射换一台主机就错了。
//!
//! # 轴的取值约定
//!
//! - 摇杆轴（`LeftX` / `LeftY` / `RightX` / `RightY`）落在 `-1.0..=1.0`；
//!   **`+Y` 是"上/前"**（与键盘 `W` 的正方向一致，见 [`crate::Input::axis`]）。
//!   手柄驱动普遍也是 `+Y = 上`，所以这里不做翻转 —— 翻转要在两个方向都
//!   一致地做，漏一处就是"上下颠倒"。
//! - 扳机轴（`LeftTrigger` / `RightTrigger`）落在 `0.0..=1.0`（松开 = 0）。

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// 轴的数量（数组下标 = [`GamepadAxis`] 的 `as usize`）。
pub const AXIS_COUNT: usize = 6;

/// 手柄上的语义按键。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum GamepadButton {
    /// 下（Xbox A / PS ✕）。
    South,
    /// 右（Xbox B / PS ○）。
    East,
    /// 左（Xbox X / PS □）。
    West,
    /// 上（Xbox Y / PS △）。
    North,
    LeftBumper,
    RightBumper,
    /// 左扳机**数字化**（按下阈值由驱动决定，通常 ~50%）。
    ///
    /// 想要"按压力度"请绑 [`GamepadAxis::LeftTrigger`]。
    LeftTrigger,
    RightTrigger,
    Select,
    Start,
    Guide,
    LeftStick,
    RightStick,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
}

impl GamepadButton {
    /// 稳定的**文本名**（`input.ron`、脚本 API、Inspector 共用同一套）。
    ///
    /// 名字一旦发布就不能改：项目文件里存的是它，脚本里传的也是它。
    pub fn name(self) -> &'static str {
        match self {
            GamepadButton::South => "south",
            GamepadButton::East => "east",
            GamepadButton::West => "west",
            GamepadButton::North => "north",
            GamepadButton::LeftBumper => "left_bumper",
            GamepadButton::RightBumper => "right_bumper",
            GamepadButton::LeftTrigger => "left_trigger",
            GamepadButton::RightTrigger => "right_trigger",
            GamepadButton::Select => "select",
            GamepadButton::Start => "start",
            GamepadButton::Guide => "guide",
            GamepadButton::LeftStick => "left_stick",
            GamepadButton::RightStick => "right_stick",
            GamepadButton::DPadUp => "dpad_up",
            GamepadButton::DPadDown => "dpad_down",
            GamepadButton::DPadLeft => "dpad_left",
            GamepadButton::DPadRight => "dpad_right",
        }
    }

    /// UI 上显示的名字（`Pad A` / `Pad LB`）。
    pub fn display_name(self) -> &'static str {
        match self {
            GamepadButton::South => "Pad A",
            GamepadButton::East => "Pad B",
            GamepadButton::West => "Pad X",
            GamepadButton::North => "Pad Y",
            GamepadButton::LeftBumper => "Pad LB",
            GamepadButton::RightBumper => "Pad RB",
            GamepadButton::LeftTrigger => "Pad LT",
            GamepadButton::RightTrigger => "Pad RT",
            GamepadButton::Select => "Pad Back",
            GamepadButton::Start => "Pad Start",
            GamepadButton::Guide => "Pad Guide",
            GamepadButton::LeftStick => "Pad LS",
            GamepadButton::RightStick => "Pad RS",
            GamepadButton::DPadUp => "Pad Up",
            GamepadButton::DPadDown => "Pad Down",
            GamepadButton::DPadLeft => "Pad Left",
            GamepadButton::DPadRight => "Pad Right",
        }
    }

    /// 全部按键（后端播种初始状态、UI 列全表都用它）。
    pub const ALL: [GamepadButton; 17] = [
        GamepadButton::South,
        GamepadButton::East,
        GamepadButton::West,
        GamepadButton::North,
        GamepadButton::LeftBumper,
        GamepadButton::RightBumper,
        GamepadButton::LeftTrigger,
        GamepadButton::RightTrigger,
        GamepadButton::Select,
        GamepadButton::Start,
        GamepadButton::Guide,
        GamepadButton::LeftStick,
        GamepadButton::RightStick,
        GamepadButton::DPadUp,
        GamepadButton::DPadDown,
        GamepadButton::DPadLeft,
        GamepadButton::DPadRight,
    ];

    /// 按名字查符号（脚本 API 入口）。
    ///
    /// 接受几种常见别名：`a`/`b`/`x`/`y`（按 Xbox 字母）、`lb`/`rb`/`lt`/`rt`、
    /// `back`、`l3`/`r3`。别名只是**便利**，落盘一律写规范名。
    pub fn from_name(name: &str) -> Option<Self> {
        let key = name.trim().to_ascii_lowercase().replace(['-', ' '], "_");
        Some(match key.as_str() {
            "south" | "a" => GamepadButton::South,
            "east" | "b" => GamepadButton::East,
            "west" | "x" => GamepadButton::West,
            "north" | "y" => GamepadButton::North,
            "left_bumper" | "lb" => GamepadButton::LeftBumper,
            "right_bumper" | "rb" => GamepadButton::RightBumper,
            "left_trigger" | "lt" => GamepadButton::LeftTrigger,
            "right_trigger" | "rt" => GamepadButton::RightTrigger,
            "select" | "back" => GamepadButton::Select,
            "start" => GamepadButton::Start,
            "guide" | "mode" => GamepadButton::Guide,
            "left_stick" | "l3" => GamepadButton::LeftStick,
            "right_stick" | "r3" => GamepadButton::RightStick,
            "dpad_up" | "up" => GamepadButton::DPadUp,
            "dpad_down" | "down" => GamepadButton::DPadDown,
            "dpad_left" | "left" => GamepadButton::DPadLeft,
            "dpad_right" | "right" => GamepadButton::DPadRight,
            _ => return None,
        })
    }
}

/// 手柄的一根轴。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum GamepadAxis {
    /// 左摇杆水平（右为正）。
    LeftX,
    /// 左摇杆垂直（**上为正**）。
    LeftY,
    /// 右摇杆水平。
    RightX,
    /// 右摇杆垂直（上为正）。
    RightY,
    /// 左扳机（0 = 松开，1 = 到底）。
    LeftTrigger,
    /// 右扳机（0 = 松开，1 = 到底）。
    RightTrigger,
}

impl GamepadAxis {
    pub fn name(self) -> &'static str {
        match self {
            GamepadAxis::LeftX => "left_x",
            GamepadAxis::LeftY => "left_y",
            GamepadAxis::RightX => "right_x",
            GamepadAxis::RightY => "right_y",
            GamepadAxis::LeftTrigger => "left_trigger",
            GamepadAxis::RightTrigger => "right_trigger",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            GamepadAxis::LeftX => "Pad LX",
            GamepadAxis::LeftY => "Pad LY",
            GamepadAxis::RightX => "Pad RX",
            GamepadAxis::RightY => "Pad RY",
            GamepadAxis::LeftTrigger => "Pad LT",
            GamepadAxis::RightTrigger => "Pad RT",
        }
    }

    pub const ALL: [GamepadAxis; AXIS_COUNT] = [
        GamepadAxis::LeftX,
        GamepadAxis::LeftY,
        GamepadAxis::RightX,
        GamepadAxis::RightY,
        GamepadAxis::LeftTrigger,
        GamepadAxis::RightTrigger,
    ];

    /// 按名字查符号（脚本 API 入口，别名同按键）。
    pub fn from_name(name: &str) -> Option<Self> {
        let key = name.trim().to_ascii_lowercase().replace(['-', ' '], "_");
        Some(match key.as_str() {
            "left_x" | "lx" => GamepadAxis::LeftX,
            "left_y" | "ly" => GamepadAxis::LeftY,
            "right_x" | "rx" => GamepadAxis::RightX,
            "right_y" | "ry" => GamepadAxis::RightY,
            "left_trigger" | "lt" => GamepadAxis::LeftTrigger,
            "right_trigger" | "rt" => GamepadAxis::RightTrigger,
            _ => return None,
        })
    }

    /// 扳机轴是单极（0..1），摇杆轴是双极（-1..1）。
    ///
    /// 死区与"按压力度换算"都要看这一点：给扳机做双极重标定会把松开
    /// 判成 -1，于是"价值为 0 时不触发"变成"一直触发"。
    pub fn is_trigger(self) -> bool {
        matches!(self, GamepadAxis::LeftTrigger | GamepadAxis::RightTrigger)
    }
}

/// 死区重标定：把 `deadzone` 以内的值压成 0，其余线性拉伸到满量程。
///
/// # 为什么要拉伸而不是直接截断
///
/// 直接截断（`v.abs() < dz => 0`）会让"刚好推出死区"时的值仍然接近 0，
/// 结果是摇杆推到底只有 0.85 —— 角色永远跑不到最高速，而且玩家会觉得
/// 摇杆"发闷"。重标定保证推到底就是 1.0。
///
/// 双极轴按**绝对值**做（左右对称）；扳机是单极，`0..dz` 压成 0。
pub fn apply_deadzone(value: f32, deadzone: f32) -> f32 {
    let v = if value.is_finite() { value } else { 0.0 };
    let dz = deadzone.clamp(0.0, 0.99);
    let mag = v.abs();
    if mag <= dz {
        return 0.0;
    }
    let scaled = (mag - dz) / (1.0 - dz);
    (scaled.min(1.0)).copysign(v)
}

/// 单个手柄的状态。
///
/// 与键盘完全同构（按住 / 本帧按下 / 本帧松开 + 累积式边沿），这样
/// 动作映射里两类设备可以走同一段判断逻辑。
#[derive(Debug, Clone, Default)]
pub struct PadState {
    /// 设备名（诊断与 UI 用；空串 = 后端没给名字）。
    pub name: String,
    buttons: HashSet<GamepadButton>,
    pressed: HashSet<GamepadButton>,
    released: HashSet<GamepadButton>,
    /// 当前轴值。
    axes: [f32; AXIS_COUNT],
    /// 上一帧末的轴值。扳机"当按键用"时靠它判"刚越过阈值"。
    prev_axes: [f32; AXIS_COUNT],
    /// 取消边沿独立保存：同帧恢复焦点不能抹掉已经发生的释放。
    cancelled_axes: Option<[f32; AXIS_COUNT]>,
    unfocused: bool,
}

impl PadState {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    /// 注入一个按键事件（重复按下与键盘同款去重）。
    pub fn handle_button(&mut self, button: GamepadButton, down: bool) {
        if self.unfocused {
            return;
        }
        if down {
            if self.buttons.insert(button) {
                self.pressed.insert(button);
            }
        } else if self.buttons.remove(&button) {
            self.released.insert(button);
        }
    }

    /// 注入一个轴值。非有限值与越界值在这里夹掉 —— 后端给的脏数据不该
    /// 传进游戏逻辑（NaN 会让相机 / 角色位置整帧变 NaN）。
    pub fn handle_axis(&mut self, axis: GamepadAxis, value: f32) {
        let v = if value.is_finite() { value } else { 0.0 };
        let v = if axis.is_trigger() {
            v.clamp(0.0, 1.0)
        } else {
            v.clamp(-1.0, 1.0)
        };
        self.axes[axis as usize] = v;
    }

    pub fn end_frame(&mut self) {
        self.pressed.clear();
        self.released.clear();
        self.cancelled_axes = None;
        self.prev_axes = if self.unfocused {
            [0.0; AXIS_COUNT]
        } else {
            self.axes
        };
    }

    /// 清掉"按住"并生成取消边沿，但保留轴的物理位置。
    ///
    /// 为什么要保留轴：轴是**物理位置**，松开不需要事件；把轴归零会让
    /// "窗口失焦时摇杆还推着"变成"回来之后摇杆突然归中再跳回来"。
    pub fn release_all_buttons(&mut self) {
        self.released.extend(self.buttons.drain());
        self.pressed.clear();
    }

    pub(crate) fn set_focused(&mut self, focused: bool) {
        let unfocused = !focused;
        if self.unfocused == unfocused {
            return;
        }
        self.unfocused = unfocused;
        if focused {
            // 后台仍更新轴的物理读数；恢复时以最新位置为基线，避免凭空开火。
            self.prev_axes = self.axes;
        } else {
            let cancelled = self.cancelled_axes.get_or_insert([0.0; AXIS_COUNT]);
            for (i, value) in cancelled.iter_mut().enumerate() {
                *value = value.max(self.axes[i]).max(self.prev_axes[i]);
            }
            self.release_all_buttons();
        }
    }

    pub fn button_down(&self, button: GamepadButton) -> bool {
        self.buttons.contains(&button)
    }

    pub fn button_pressed(&self, button: GamepadButton) -> bool {
        self.pressed.contains(&button)
    }

    pub fn button_released(&self, button: GamepadButton) -> bool {
        self.released.contains(&button)
    }

    pub fn axis(&self, axis: GamepadAxis) -> f32 {
        if self.unfocused {
            0.0
        } else {
            self.axes[axis as usize]
        }
    }

    /// 这根轴本帧是否**刚越过**阈值（向上）。
    pub fn axis_crossed_up(&self, axis: GamepadAxis, threshold: f32) -> bool {
        let i = axis as usize;
        !self.unfocused && self.axis(axis) >= threshold && self.prev_axes[i] < threshold
    }

    /// 这根轴本帧是否**刚跌回**阈值以下。
    pub fn axis_crossed_down(&self, axis: GamepadAxis, threshold: f32) -> bool {
        let i = axis as usize;
        self.cancelled_axes.is_some_and(|axes| axes[i] >= threshold)
            || (self.axis(axis) < threshold && self.prev_axes[i] >= threshold)
    }

    /// 当前按住的键（顺序未定义，调试面板用）。
    pub fn buttons_down(&self) -> impl Iterator<Item = GamepadButton> + '_ {
        self.buttons.iter().copied()
    }

    /// 本帧是否有任何键刚按下（"按任意键继续"要认得手柄）。
    pub fn anything_pressed(&self) -> bool {
        !self.pressed.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttton_edges_are_level_and_edge_tracked() {
        let mut pad = PadState::new("test");
        pad.handle_button(GamepadButton::South, true);
        assert!(pad.button_down(GamepadButton::South));
        assert!(pad.button_pressed(GamepadButton::South));
        pad.end_frame();
        assert!(pad.button_down(GamepadButton::South), "按住要跨帧保持");
        assert!(!pad.button_pressed(GamepadButton::South), "边沿只活一帧");

        // 重复按下（驱动抖动）不能刷新边沿。
        pad.handle_button(GamepadButton::South, true);
        assert!(!pad.button_pressed(GamepadButton::South));

        pad.handle_button(GamepadButton::South, false);
        assert!(pad.button_released(GamepadButton::South));
        pad.end_frame();
        assert!(!pad.button_released(GamepadButton::South));
    }

    #[test]
    fn axis_values_are_clamped_and_nan_free() {
        let mut pad = PadState::new("test");
        pad.handle_axis(GamepadAxis::LeftX, 5.0);
        assert_eq!(pad.axis(GamepadAxis::LeftX), 1.0);
        pad.handle_axis(GamepadAxis::LeftX, f32::NAN);
        assert_eq!(pad.axis(GamepadAxis::LeftX), 0.0);
        // 扳机是单极：给负数不能变成"负压力"。
        pad.handle_axis(GamepadAxis::RightTrigger, -3.0);
        assert_eq!(pad.axis(GamepadAxis::RightTrigger), 0.0);
    }

    #[test]
    fn trigger_crossing_is_edge_triggered_at_the_binding_threshold() {
        let mut pad = PadState::new("test");
        // 旧值 0.0，新值 0.6：越过 0.35 的阈值，本帧应算"刚按下"。
        pad.handle_axis(GamepadAxis::RightTrigger, 0.6);
        assert!(pad.axis_crossed_up(GamepadAxis::RightTrigger, 0.35));
        assert!(!pad.axis_crossed_up(GamepadAxis::RightTrigger, 0.7));
        pad.end_frame();
        // 同一帧之后再来一次同样的值：还在阈值之上，但不算新的一次。
        pad.handle_axis(GamepadAxis::RightTrigger, 0.6);
        assert!(!pad.axis_crossed_up(GamepadAxis::RightTrigger, 0.35));

        pad.handle_axis(GamepadAxis::RightTrigger, 0.0);
        assert!(pad.axis_crossed_down(GamepadAxis::RightTrigger, 0.35));
    }

    #[test]
    fn deadzone_rescales_instead_of_clipping() {
        // 死区以内归零。
        assert_eq!(apply_deadzone(0.1, 0.2), 0.0);
        assert_eq!(apply_deadzone(-0.1, 0.2), 0.0);
        // 刚出死区从 0 开始连续（不留台阶）。
        assert!(apply_deadzone(0.2001, 0.2).abs() < 1e-3);
        // 推到底必须是满量程 —— 这是"重标定"与"直接截断"的分界。
        assert_eq!(apply_deadzone(1.0, 0.2), 1.0);
        assert_eq!(apply_deadzone(-1.0, 0.2), -1.0);
        // 负值对称。
        assert!((apply_deadzone(-0.6, 0.2) + apply_deadzone(0.6, 0.2)).abs() < 1e-6);
        // NaN 不传染。
        assert_eq!(apply_deadzone(f32::NAN, 0.2), 0.0);
    }

    #[test]
    fn button_names_round_trip_and_accept_aliases() {
        for button in GamepadButton::ALL {
            assert_eq!(GamepadButton::from_name(button.name()), Some(button));
        }
        assert_eq!(
            GamepadButton::from_name("A"),
            Some(GamepadButton::South),
            "Xbox 字母别名要能用（大小写无关）"
        );
        assert_eq!(
            GamepadButton::from_name("LB"),
            Some(GamepadButton::LeftBumper)
        );
        assert_eq!(GamepadButton::from_name("nope"), None);
        for axis in GamepadAxis::ALL {
            assert_eq!(GamepadAxis::from_name(axis.name()), Some(axis));
        }
    }
}
