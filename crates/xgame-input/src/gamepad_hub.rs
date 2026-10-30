//! 手柄后端：把 `gilrs` 的事件翻译成 [`InputState`] 的手柄状态。
//!
//! # 为什么单独一层、还藏在 feature 后面
//!
//! `xgame-input` 的核心（[`InputState`] / [`InputMap`]）**不依赖任何窗口库或
//! 设备库**：那是"可测试、可重放、可接网络输入"的前提。`gilrs` 是一个会开
//! 后台线程、在 Windows 上要初始化 COM 的真实设备后端 —— 它只该出现在
//! 真正接窗口的两个宿主（编辑器、独立游戏）里。
//!
//! 于是：映射与状态在核心层（纯数据、单测覆盖），本模块只做**翻译**。
//! 没有手柄的环境（CI、无头截图、`--check`）根本不会构造它。
//!
//! # 一次 `poll` 做三件事
//!
//! 1. 把 `gilrs` 队列里的事件全部排空（它同时负责让后台线程继续跑）；
//! 2. 给"启动前就插着"的手柄补一次登记 —— `gilrs` 只在**插拔**时发
//!    `Connected`，进程启动时已连着的手柄不会再有那个事件；
//! 3. 把事件翻成 [`InputState::handle_pad_button`] / `handle_pad_axis`。
//!
//! # 扳机的两套约定
//!
//! `gilrs` 的轴统一声明在 `-1.0..=1.0`，但各平台后端的扳机实际给的是
//! `0.0..=1.0`（XInput）或 `-1.0..=1.0`（部分 evdev 设备）。这里统一成
//! "松开 = 0、扣到底 = 1"：负值走 `(v + 1) / 2`，非负值原样用。两种后端
//! 都不会出现"松开却读到 0.5"这种错位。

use std::collections::HashMap;

use gilrs::{Axis, Button, EventType, Gilrs};

use crate::pad::{GamepadAxis, GamepadButton};
use crate::InputState;

/// `gilrs` 按键 → 本引擎的语义按键。
///
/// ⚠ **`gilrs` 的 `LeftTrigger` 是"肩键"不是"扳机"**。它沿用 Linux evdev 的
/// 编号：`BTN_TL`（L1 / LB）叫 `LeftTrigger`，`BTN_TL2`（L2 / LT，那个有行程的）
/// 才叫 `LeftTrigger2`。Windows 后端把 XInput 的 `LEFT_SHOULDER` 报成
/// `BTN_TL`、把模拟扳机报成 `BTN_LT2` —— 按名字直译会让"按肩键"被当成
/// "扣扳机"，两边全都错位。这里按**物理部件**翻译。
const BUTTON_MAP: [(Button, GamepadButton); 17] = [
    (Button::South, GamepadButton::South),
    (Button::East, GamepadButton::East),
    (Button::West, GamepadButton::West),
    (Button::North, GamepadButton::North),
    (Button::LeftTrigger, GamepadButton::LeftBumper),
    (Button::RightTrigger, GamepadButton::RightBumper),
    // 扳机的**数字化**上报（Windows / XInput 走这条；模拟量走 `ButtonValueChanged`）。
    (Button::LeftTrigger2, GamepadButton::LeftTrigger),
    (Button::RightTrigger2, GamepadButton::RightTrigger),
    (Button::Select, GamepadButton::Select),
    (Button::Start, GamepadButton::Start),
    (Button::Mode, GamepadButton::Guide),
    (Button::LeftThumb, GamepadButton::LeftStick),
    (Button::RightThumb, GamepadButton::RightStick),
    (Button::DPadUp, GamepadButton::DPadUp),
    (Button::DPadDown, GamepadButton::DPadDown),
    (Button::DPadLeft, GamepadButton::DPadLeft),
    (Button::DPadRight, GamepadButton::DPadRight),
];

/// 触发器的模拟量从哪个按键编号读（见上一条的说明）。
const TRIGGER_BUTTONS: [(Button, GamepadAxis); 2] = [
    (Button::LeftTrigger2, GamepadAxis::LeftTrigger),
    (Button::RightTrigger2, GamepadAxis::RightTrigger),
];

/// `gilrs` 轴 → 本引擎的轴。`LeftZ` / `RightZ` 是扳机（见模块头的说明）。
const AXIS_MAP: [(Axis, GamepadAxis); 6] = [
    (Axis::LeftStickX, GamepadAxis::LeftX),
    (Axis::LeftStickY, GamepadAxis::LeftY),
    (Axis::RightStickX, GamepadAxis::RightX),
    (Axis::RightStickY, GamepadAxis::RightY),
    (Axis::LeftZ, GamepadAxis::LeftTrigger),
    (Axis::RightZ, GamepadAxis::RightTrigger),
];

/// 把扳机读数统一成"松开 = 0、到底 = 1"（见模块头）。
fn normalize_trigger(value: f32) -> f32 {
    if value.is_finite() {
        if value < 0.0 {
            (value + 1.0) * 0.5
        } else {
            value
        }
    } else {
        0.0
    }
}

/// 手柄后端。宿主拥有一份，每帧 `poll` 一次。
pub struct GamepadHub {
    gilrs: Gilrs,
    /// `gilrs` 的 id → 本引擎的 id（0 起的小整数，稳定到断开为止）。
    ///
    /// 为什么不直接把 `gilrs::GamepadId` 当外部 id：它是新类型且不保证与
    /// 下标连续，脚本 / 存档 / 日志里出现 `GamepadId(3)` 既不好读也不好稳定
    /// 序列化。用自己发的号，语义是"这台机器上第几次接上手柄"。
    slots: HashMap<gilrs::GamepadId, u32>,
    next_slot: u32,
}

impl GamepadHub {
    /// 初始化后端。失败时返回原因（宿主记一条 warn 就继续跑 —— 没有手柄
    /// 不该让游戏启动不起来）。
    pub fn new() -> Result<Self, String> {
        let gilrs = Gilrs::new().map_err(|e| format!("手柄后端初始化失败：{e}"))?;
        Ok(Self {
            gilrs,
            slots: HashMap::new(),
            next_slot: 0,
        })
    }

    /// 排空事件队列并写进 `state`。**每帧调用一次**（不调用 = 手柄完全没反应，
    /// 连插拔都收不到）。
    pub fn poll(&mut self, state: &mut InputState) {
        // 启动前就插着的手柄不会有 `Connected` 事件，先补一次登记。
        let already: Vec<gilrs::GamepadId> = self
            .gilrs
            .gamepads()
            .map(|(id, _)| id)
            .filter(|id| !self.slots.contains_key(id))
            .collect();
        for id in already {
            self.connect(id, state);
        }

        while let Some(event) = self.gilrs.next_event() {
            let id = event.id;
            let kind = event.event;
            self.apply(id, kind, state);
        }
    }

    /// 手柄连上（或启动时就存在）：登记 + 用当前物理状态播种。
    ///
    /// 播种是必须的：玩家可能在插着手柄的同时就按住了摇杆，那种情况下
    /// 没有"变化事件"可听，只有读一次当前状态才知道。
    fn connect(&mut self, id: gilrs::GamepadId, state: &mut InputState) {
        let slot = self.ensure_slot(id);
        let gamepad = self.gilrs.gamepad(id);
        let name = gamepad.name().to_owned();
        state.pad_connected(slot, &name);

        // 用 `Gamepad` 上的查询而不是 `State`：前者会**过一遍设备映射表**
        // （SDL 映射），后者只认原始 evdev 编号 —— 手柄千奇百怪，绕开映射
        // 等于只支持"恰好与 Xbox 布局一致"的那几款。
        for (native, button) in BUTTON_MAP {
            let down = gamepad
                .button_data(native)
                .is_some_and(|data| data.is_pressed());
            state.handle_pad_button(slot, button, down);
        }
        for (native, axis) in AXIS_MAP {
            if let Some(data) = gamepad.axis_data(native) {
                let value = if axis.is_trigger() {
                    normalize_trigger(data.value())
                } else {
                    data.value()
                };
                state.handle_pad_axis(slot, axis, value);
            }
        }
        // 扳机的模拟量在 Windows 后端不在 `axis_data` 里，而在按键的
        // `value()` 上（0..1）。两条都读：轴有值以轴为准（evdev 只有轴），
        // 否则用按键的值。
        for (native, axis) in TRIGGER_BUTTONS {
            if state.pad_axis(axis) > 0.0 {
                continue;
            }
            if let Some(data) = gamepad.button_data(native) {
                state.handle_pad_axis(slot, axis, data.value().clamp(0.0, 1.0));
            }
        }
    }

    fn ensure_slot(&mut self, id: gilrs::GamepadId) -> u32 {
        if let Some(&slot) = self.slots.get(&id) {
            return slot;
        }
        let slot = self.next_slot;
        self.next_slot = self.next_slot.wrapping_add(1);
        self.slots.insert(id, slot);
        slot
    }

    fn apply(&mut self, id: gilrs::GamepadId, kind: EventType, state: &mut InputState) {
        match kind {
            EventType::Connected => self.connect(id, state),
            EventType::Disconnected => {
                if let Some(slot) = self.slots.remove(&id) {
                    state.pad_disconnected(slot);
                }
            }
            EventType::ButtonPressed(native, _) => {
                if let Some(slot) = self.slot_of(id) {
                    for (from, to) in BUTTON_MAP {
                        if from == native {
                            state.handle_pad_button(slot, to, true);
                        }
                    }
                }
            }
            EventType::ButtonReleased(native, _) => {
                if let Some(slot) = self.slot_of(id) {
                    for (from, to) in BUTTON_MAP {
                        if from == native {
                            state.handle_pad_button(slot, to, false);
                        }
                    }
                }
            }
            EventType::AxisChanged(native, value, _) => {
                if let Some(slot) = self.slot_of(id) {
                    for (from, to) in AXIS_MAP {
                        if from == native {
                            let v = if to.is_trigger() {
                                normalize_trigger(value)
                            } else {
                                value
                            };
                            state.handle_pad_axis(slot, to, v);
                        }
                    }
                }
            }
            // 扳机的模拟量（Windows 的数字化按键也带 0..1 的值）。
            EventType::ButtonChanged(native, value, _) => {
                if let Some(slot) = self.slot_of(id) {
                    for (from, axis) in TRIGGER_BUTTONS {
                        if from == native {
                            state.handle_pad_axis(slot, axis, value.clamp(0.0, 1.0));
                        }
                    }
                }
            }
            // 其余事件（震动反馈等）：本层不消费。
            _ => {}
        }
    }

    fn slot_of(&self, id: gilrs::GamepadId) -> Option<u32> {
        self.slots.get(&id).copied()
    }

    /// 已登记的 `gilrs` 设备数（诊断用）。
    pub fn device_count(&self) -> usize {
        self.slots.len()
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_trigger;

    #[test]
    fn trigger_conventions_both_land_on_zero_to_one() {
        // XInput 风格：松开 0、到底 1。
        assert_eq!(normalize_trigger(0.0), 0.0);
        assert_eq!(normalize_trigger(1.0), 1.0);
        // evdev 风格：松开 -1、到底 1。
        assert_eq!(normalize_trigger(-1.0), 0.0);
        assert_eq!(normalize_trigger(0.0), 0.0);
        // 非法值不传染。
        assert_eq!(normalize_trigger(f32::NAN), 0.0);
    }
}
