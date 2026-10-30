//! 输入设备的原始状态。
//!
//! [`InputState`] 只负责"谁被按下了、指针在哪"，**不做**任何动作映射 ——
//! 那是 [`crate::InputMap`] 的职责。这样分层的好处是：录键、重放、网络同步
//! 都可以直接读写这个结构，不关心游戏怎么用这些按键。
//!
//! # 帧语义
//!
//! 输入分三类，生命周期各不相同：
//!
//! | 查询 | 含义 | 何时失效 |
//! |---|---|---|
//! | `key_down` | 这一刻是否按住 | 松开时 |
//! | `key_pressed` | **本帧**是否刚按下 | 帧末 [`InputState::end_frame`] |
//! | `key_released` | **本帧**是否刚松开 | 帧末 [`InputState::end_frame`] |
//!
//! 「本帧」的实现方式是**累积 + 帧末清空**，而不是"对比上一帧快照"。
//! 原因很实际：事件在两帧之间随时到达（winit 可以在一次 `AboutToWait` 里
//! 塞进好几个按键事件），如果只有一帧一帧的快照，中间那一帧就永远为 false。
//! 累积则不会漏 —— 一个事件只要发生过，就一定被本帧的 `pressed` 读到。

use std::collections::{BTreeMap, HashSet};

use xgame_math::glam::Vec2;

use crate::key::{KeyCode, MouseButton};
use crate::pad::{GamepadAxis, GamepadButton, PadState};

/// 一帧内允许缓存的文本输入长度上限。
///
/// 防止"按住一个键不放 + 输入法疯狂上屏"把字符串撑到不可控的大小。
const MAX_TEXT_LEN: usize = 1024;

/// 键鼠的原始状态。
#[derive(Debug, Clone, Default)]
pub struct InputState {
    keys_down: HashSet<KeyCode>,
    keys_pressed: HashSet<KeyCode>,
    keys_released: HashSet<KeyCode>,

    buttons_down: HashSet<MouseButton>,
    buttons_pressed: HashSet<MouseButton>,
    buttons_released: HashSet<MouseButton>,

    mouse_pos: Vec2,
    /// 指针是否在窗口内：收到过 `CursorMoved`、且没有 `CursorLeft`。
    ///
    /// 为什么需要这个标志：`mouse_pos` 的初值是 `(0,0)`，而 `(0,0)` 是
    /// 一个合法位置 —— 分不清"指针在左上角"与"还没有指针事件"。UI 命中
    /// 测试拿它当"指针在不在"，会在窗口刚打开时把左上角的元素点亮。
    cursor_seen: bool,
    /// 本帧鼠标的位移。
    ///
    /// 有两个来源，都往里累加：
    /// - `handle_cursor_moved`：由光标绝对坐标求差；
    /// - `handle_mouse_motion`：窗口库给的原始相对位移（鼠标被锁定时只有它可用）。
    mouse_delta: Vec2,
    scroll: Vec2,

    text: String,
    focused: bool,

    /// 已连接手柄（id 由后端分配，稳定到断开为止）。
    ///
    /// 用 `BTreeMap` 而不是 `HashMap`：动作查询要"任一手柄成立即可"，
    /// 遍历顺序虽然不影响结果，但**诊断与日志**会带上顺序，`BTreeMap`
    /// 至少让同一台机器的两份日志可比较。
    pads: BTreeMap<u32, PadState>,
}

impl InputState {
    pub fn new() -> Self {
        Self {
            focused: true,
            mouse_pos: Vec2::ZERO,
            ..Default::default()
        }
    }

    // -----------------------------------------------------------------------
    // 事件注入（平台层调用；测试里也可以直接调）
    // -----------------------------------------------------------------------

    /// 注入一次键盘事件。
    pub fn handle_key(&mut self, code: KeyCode, pressed: bool) {
        if pressed {
            // `insert` 的返回值区分了"第一次按下"和"重复按下"：
            // 操作系统的按键重复（按住 A 会持续投递 Pressed）不该被当成
            // 一帧按了十几次 —— 否则"按一下跳一次"会变成"按住就狂跳"。
            if self.keys_down.insert(code) {
                self.keys_pressed.insert(code);
            }
            self.keys_released.remove(&code);
        } else {
            if self.keys_down.remove(&code) {
                self.keys_released.insert(code);
            }
            self.keys_pressed.remove(&code);
        }
    }

    /// 注入一次鼠标按键事件。
    pub fn handle_mouse_button(&mut self, button: MouseButton, pressed: bool) {
        if pressed {
            if self.buttons_down.insert(button) {
                self.buttons_pressed.insert(button);
            }
            self.buttons_released.remove(&button);
        } else {
            if self.buttons_down.remove(&button) {
                self.buttons_released.insert(button);
            }
            self.buttons_pressed.remove(&button);
        }
    }

    /// 光标移动到一个**绝对**位置。
    pub fn handle_cursor_moved(&mut self, pos: Vec2) {
        // 位移的语义保持原样（与 `mouse_pos` 的初值有关，既有测试锁着）：
        // 这里只额外记录"指针真的出现过"，供 UI 判断悬停。
        self.mouse_delta += pos - self.mouse_pos;
        self.mouse_pos = pos;
        self.cursor_seen = true;
    }

    /// 光标离开窗口（winit 的 `CursorLeft`）。
    ///
    /// 位置保留在最后一点：窗口重新获得指针时 `handle_cursor_moved` 会用
    /// 新位置覆盖它。`has_cursor` 转 false，UI 悬停随之清空。
    pub fn handle_cursor_left(&mut self) {
        self.cursor_seen = false;
    }

    /// 原始相对位移（鼠标被捕获 / 锁定时使用）。
    pub fn handle_mouse_motion(&mut self, delta: Vec2) {
        self.mouse_delta += delta;
    }

    /// 滚轮 / 触控板滚动。`y` 为正表示向上滚。
    pub fn handle_scroll(&mut self, delta: Vec2) {
        self.scroll += delta;
    }

    // -- 手柄（后端把插拔与事件翻译成这几个入口）--

    /// 登记一台手柄（后端在 `Connected` 事件里调用）。
    ///
    /// 重复登记只更新名字，不重置状态：后端重连时可能连发两次 `Connected`，
    /// 第二次不该把玩家的按键清掉。
    pub fn pad_connected(&mut self, pad: u32, name: &str) {
        match self.pads.get_mut(&pad) {
            Some(existing) => existing.name = name.to_owned(),
            None => {
                self.pads.insert(pad, PadState::new(name));
            }
        }
    }

    /// 拔掉一台手柄。它这一帧的边沿信号一并消失 —— 断开之后不该再有"刚按下"。
    pub fn pad_disconnected(&mut self, pad: u32) {
        self.pads.remove(&pad);
    }

    /// 注入一次手柄按键事件。
    ///
    /// 没登记过的手柄会自动补一条空记录：后端的 `Connected` 事件可能早于
    /// 本帧第一帧输入到达（尤其是在 poll 里第一次 drain 的时候），漏一条
    /// 插拔事件不该让按钮永远读不到。
    pub fn handle_pad_button(&mut self, pad: u32, button: GamepadButton, down: bool) {
        self.pads
            .entry(pad)
            .or_default()
            .handle_button(button, down);
    }

    /// 注入一次手柄轴值。
    pub fn handle_pad_axis(&mut self, pad: u32, axis: GamepadAxis, value: f32) {
        self.pads.entry(pad).or_default().handle_axis(axis, value);
    }

    /// 文本输入（输入法上屏、字符键）。
    pub fn handle_text(&mut self, text: &str) {
        if self.text.len() + text.len() > MAX_TEXT_LEN {
            return;
        }
        self.text.push_str(text);
    }

    /// 窗口焦点变化。
    ///
    /// 失焦时必须**清空所有按下状态**：否则切出去之后系统不再投递"松开"事件，
    /// 回来时那些键会永远卡在"按住"上（典型的症状：切出去再切回来，
    /// 角色一直往一个方向走）。
    pub fn set_focused(&mut self, focused: bool) {
        if self.focused && !focused {
            self.keys_down.clear();
            self.buttons_down.clear();
            // 手柄同理：窗口失焦后后端仍会继续上报（它是后台轮询），但游戏
            // 不该在"没焦点"时继续读到一个按住的跳跃键。
            for pad in self.pads.values_mut() {
                pad.release_all_buttons();
            }
        }
        self.focused = focused;
    }

    /// 帧末清理。
    ///
    /// 调用时机：**所有**读输入的代码都跑完之后。清掉的是"仅本帧有效"的部分，
    /// 按住状态与光标位置保留。
    pub fn end_frame(&mut self) {
        self.keys_pressed.clear();
        self.keys_released.clear();
        self.buttons_pressed.clear();
        self.buttons_released.clear();
        self.mouse_delta = Vec2::ZERO;
        self.scroll = Vec2::ZERO;
        self.text.clear();
        for pad in self.pads.values_mut() {
            pad.end_frame();
        }
    }

    /// 强制清空全部状态（场景重载 / 切换时用，避免上一个场景的残留输入）。
    ///
    /// **手柄的"已连接"名单保留**：那是硬件事实，不是游戏状态。清掉它之后
    /// 后端只在"下一次插拔事件"才会重新登记，于是切一次场景摇杆就失灵 ——
    /// 而且重启后第一次运行（没有插拔事件）永远没有手柄。
    pub fn reset(&mut self) {
        let pads = std::mem::take(&mut self.pads);
        *self = Self::new();
        self.pads = pads;
        for pad in self.pads.values_mut() {
            pad.release_all_buttons();
        }
    }

    // -----------------------------------------------------------------------
    // 查询
    // -----------------------------------------------------------------------

    pub fn key_down(&self, code: KeyCode) -> bool {
        self.keys_down.contains(&code)
    }

    pub fn key_pressed(&self, code: KeyCode) -> bool {
        self.keys_pressed.contains(&code)
    }

    pub fn key_released(&self, code: KeyCode) -> bool {
        self.keys_released.contains(&code)
    }

    pub fn mouse_down(&self, button: MouseButton) -> bool {
        self.buttons_down.contains(&button)
    }

    pub fn mouse_pressed(&self, button: MouseButton) -> bool {
        self.buttons_pressed.contains(&button)
    }

    pub fn mouse_released(&self, button: MouseButton) -> bool {
        self.buttons_released.contains(&button)
    }

    // -----------------------------------------------------------------------
    // 手柄查询
    //
    // 语义统一是"**任意一台手柄**成立即可"（Unity / Godot 的默认行为）：
    // 沙发上的第二个手柄按 A 也能触发"开始游戏"，作者不必先知道是哪台。
    // 需要区分设备时用带 `pad` 参数的那组。
    // -----------------------------------------------------------------------

    /// 已连接的手柄 id（升序）。
    pub fn pad_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.pads.keys().copied()
    }

    pub fn pad_count(&self) -> usize {
        self.pads.len()
    }

    pub fn pad_name(&self, pad: u32) -> Option<&str> {
        self.pads.get(&pad).map(|p| p.name.as_str())
    }

    pub fn pad_button_down(&self, button: GamepadButton) -> bool {
        self.pads.values().any(|p| p.button_down(button))
    }

    pub fn pad_button_pressed(&self, button: GamepadButton) -> bool {
        self.pads.values().any(|p| p.button_pressed(button))
    }

    pub fn pad_button_released(&self, button: GamepadButton) -> bool {
        self.pads.values().any(|p| p.button_released(button))
    }

    /// 轴的当前值：取**绝对值最大**的那台手柄。
    ///
    /// 为什么不是"第一台"或"求和"：多人同屏时"两个人都推满"不该算成 2.0
    /// （越界），也不该因为插了第二台就把第一台的读数覆盖掉。取绝对值最大
    /// 与"谁的输入更强就用谁的"直觉一致，且手柄 id 升序遍历保证平手时结果确定。
    pub fn pad_axis(&self, axis: GamepadAxis) -> f32 {
        let mut best = 0.0f32;
        for pad in self.pads.values() {
            let v = pad.axis(axis);
            if v.abs() > best.abs() {
                best = v;
            }
        }
        best
    }

    pub fn pad_axis_of(&self, pad: u32, axis: GamepadAxis) -> f32 {
        self.pads.get(&pad).map_or(0.0, |p| p.axis(axis))
    }

    /// 某根轴本帧是否刚越过阈值（任意一台手柄）。
    pub fn pad_axis_crossed_up(&self, axis: GamepadAxis, threshold: f32) -> bool {
        self.pads.values().any(|p| {
            // 跨手柄比较"上一帧值"没有意义（每台各有一份），所以逐台判边沿
            // 再取或 —— 两台手柄同时压下扳机只算一次，符合"动作"的语义。
            p.axis_crossed_up(axis, threshold)
        })
    }

    pub fn pad_axis_crossed_down(&self, axis: GamepadAxis, threshold: f32) -> bool {
        self.pads
            .values()
            .any(|p| p.axis_crossed_down(axis, threshold))
    }

    /// 本帧是否有任何键刚按下（"按任意键继续"）。
    ///
    /// 手柄按键也算 —— 主机玩家看到"按任意键"时手里只有手柄。
    pub fn any_key_pressed(&self) -> bool {
        !self.keys_pressed.is_empty() || self.pads.values().any(|p| p.anything_pressed())
    }

    /// 当前按住的键（顺序未定义，用于调试面板）。
    pub fn keys_down(&self) -> impl Iterator<Item = KeyCode> + '_ {
        self.keys_down.iter().copied()
    }

    /// 指针是否在窗口内（见 `cursor_seen` 的说明）。
    pub fn has_cursor(&self) -> bool {
        self.cursor_seen
    }

    pub fn mouse_pos(&self) -> Vec2 {
        self.mouse_pos
    }

    pub fn mouse_delta(&self) -> Vec2 {
        self.mouse_delta
    }

    pub fn scroll(&self) -> Vec2 {
        self.scroll
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    // -- 修饰键便捷查询（动作映射里的 Combo 也走这几个）--

    pub fn shift_down(&self) -> bool {
        self.key_down(KeyCode::ShiftLeft) || self.key_down(KeyCode::ShiftRight)
    }

    pub fn ctrl_down(&self) -> bool {
        self.key_down(KeyCode::ControlLeft) || self.key_down(KeyCode::ControlRight)
    }

    pub fn alt_down(&self) -> bool {
        self.key_down(KeyCode::AltLeft) || self.key_down(KeyCode::AltRight)
    }

    pub fn super_down(&self) -> bool {
        self.key_down(KeyCode::SuperLeft) || self.key_down(KeyCode::SuperRight)
    }

    /// 精确匹配一组修饰键状态（`Combo` 绑定用）。
    pub(crate) fn modifiers_match(&self, ctrl: bool, shift: bool, alt: bool, sup: bool) -> bool {
        self.ctrl_down() == ctrl
            && self.shift_down() == shift
            && self.alt_down() == alt
            && self.super_down() == sup
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressed_is_edge_triggered_not_level() {
        let mut s = InputState::new();
        s.handle_key(KeyCode::KeyW, true);
        assert!(s.key_down(KeyCode::KeyW));
        assert!(s.key_pressed(KeyCode::KeyW));

        s.end_frame();
        // 帧末之后："按住"还在，"刚按下"已经过期。
        assert!(s.key_down(KeyCode::KeyW));
        assert!(!s.key_pressed(KeyCode::KeyW));
    }

    #[test]
    fn key_repeat_does_not_retrigger_pressed() {
        let mut s = InputState::new();
        s.handle_key(KeyCode::Space, true);
        s.end_frame();
        // 操作系统按住不放会持续投递 Pressed —— 不能当成新的一帧按下。
        s.handle_key(KeyCode::Space, true);
        assert!(!s.key_pressed(KeyCode::Space));
    }

    #[test]
    fn release_is_edge_triggered() {
        let mut s = InputState::new();
        s.handle_key(KeyCode::KeyA, true);
        s.end_frame();
        s.handle_key(KeyCode::KeyA, false);
        assert!(s.key_released(KeyCode::KeyA));
        assert!(!s.key_down(KeyCode::KeyA));
        s.end_frame();
        assert!(!s.key_released(KeyCode::KeyA));
    }

    #[test]
    fn losing_focus_releases_held_keys() {
        let mut s = InputState::new();
        s.handle_key(KeyCode::KeyD, true);
        s.set_focused(false);
        assert!(!s.key_down(KeyCode::KeyD), "失焦后不能留下卡住的按键");
    }

    #[test]
    fn mouse_delta_accumulates_and_clears() {
        let mut s = InputState::new();
        s.handle_cursor_moved(Vec2::new(10.0, 10.0));
        s.handle_cursor_moved(Vec2::new(12.0, 7.0));
        assert_eq!(s.mouse_delta(), Vec2::new(12.0, 7.0));
        assert_eq!(s.mouse_pos(), Vec2::new(12.0, 7.0));
        s.end_frame();
        assert_eq!(s.mouse_delta(), Vec2::ZERO);
        assert_eq!(s.mouse_pos(), Vec2::new(12.0, 7.0), "位置要保留，位移才清");
    }

    #[test]
    fn combo_requires_exact_modifiers() {
        let mut s = InputState::new();
        s.handle_key(KeyCode::ControlLeft, true);
        assert!(s.modifiers_match(true, false, false, false));
        assert!(!s.modifiers_match(false, false, false, false));
    }
}
