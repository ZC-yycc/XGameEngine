//! 逻辑键码与鼠标按键。
//!
//! 这里刻意**不**直接使用窗口库（winit）的类型。原因有三：
//!
//! 1. 输入层要能在没有窗口的环境里被驱动 —— 单元测试、回放系统、网络同步的
//!    输入流都只需要"某个键被按下了"，不需要一个真实的窗口；
//! 2. 换窗口库（或同时支持多个平台后端）时，改的只是那一层薄薄的翻译代码；
//! 3. 动作映射（`input.ron`）是**游戏资产**，它的键名必须稳定、可读、与
//!    窗口库的版本无关 —— 不能因为上游把 `KeyCode::SuperLeft` 改了名，
//!    玩家的配置文件就失效。
//!
//! 键码命名沿用 W3C UI Events 的 `KeyboardEvent.code`（也是 winit 使用的标准），
//! 即**物理位置**而非字符 —— 这样 AZERTY 键盘上按 `W` 物理键位，
//! 绑到 `KeyCode::KeyW` 的动作依然生效。

use serde::{Deserialize, Serialize};

/// 物理键位（布局无关）。
///
/// # 变体命名
///
/// 与 W3C `KeyboardEvent.code` 对齐：字母是 `KeyW`、数字是 `Digit1`、
/// 小键盘是 `Numpad1`。序列化成 RON 时用的是这个变体名，
/// 所以 `input.ron` 里写的是 `Key(Space)` / `Key(KeyW)` 这种稳定标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum KeyCode {
    // -- 字母（物理位置，与键盘布局无关）--
    KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM,
    KeyN, KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,

    // -- 主键盘数字 --
    Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9,

    // -- 功能键 --
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
    F13, F14, F15, F16, F17, F18, F19, F20, F21, F22, F23, F24,

    // -- 编辑键 --
    Escape,
    Enter,
    Tab,
    Space,
    Backspace,
    Delete,
    Insert,

    // -- 方向 / 导航 --
    ArrowUp, ArrowDown, ArrowLeft, ArrowRight,
    Home, End, PageUp, PageDown,

    // -- 修饰键 --
    ShiftLeft, ShiftRight,
    ControlLeft, ControlRight,
    AltLeft, AltRight,
    /// Windows 键 / Command 键。
    SuperLeft, SuperRight,
    /// 上下文菜单键。
    ContextMenu,

    // -- 标点（US 布局上的名字，物理位置为标准）--
    Backquote,
    Minus,
    Equal,
    BracketLeft,
    BracketRight,
    Backslash,
    Semicolon,
    Quote,
    Comma,
    Period,
    Slash,

    // -- 锁定键 --
    CapsLock, NumLock, ScrollLock,
    /// 打印屏幕。
    PrintScreen,
    /// 暂停 / 中断。
    Pause,

    // -- 小键盘 --
    Numpad0, Numpad1, Numpad2, Numpad3, Numpad4,
    Numpad5, Numpad6, Numpad7, Numpad8, Numpad9,
    NumpadAdd, NumpadSubtract, NumpadMultiply, NumpadDivide,
    NumpadDecimal, NumpadEnter,
}

impl KeyCode {
    /// 全部键码。
    ///
    /// 手工维护是有意的：`KeyCode` 是**文件格式的一部分**（`input.ron` 里存的是
    /// 变体名），枚举顺序一旦被 `strum` 之类的宏自动生成，就跟着声明顺序飘。
    /// 这里显式列出，顺带给"录键界面要遍历所有键"提供了一个稳定顺序。
    pub const ALL: &'static [KeyCode] = &[
        KeyCode::KeyA, KeyCode::KeyB, KeyCode::KeyC, KeyCode::KeyD, KeyCode::KeyE,
        KeyCode::KeyF, KeyCode::KeyG, KeyCode::KeyH, KeyCode::KeyI, KeyCode::KeyJ,
        KeyCode::KeyK, KeyCode::KeyL, KeyCode::KeyM, KeyCode::KeyN, KeyCode::KeyO,
        KeyCode::KeyP, KeyCode::KeyQ, KeyCode::KeyR, KeyCode::KeyS, KeyCode::KeyT,
        KeyCode::KeyU, KeyCode::KeyV, KeyCode::KeyW, KeyCode::KeyX, KeyCode::KeyY,
        KeyCode::KeyZ,
        KeyCode::Digit0, KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3,
        KeyCode::Digit4, KeyCode::Digit5, KeyCode::Digit6, KeyCode::Digit7,
        KeyCode::Digit8, KeyCode::Digit9,
        KeyCode::F1, KeyCode::F2, KeyCode::F3, KeyCode::F4, KeyCode::F5, KeyCode::F6,
        KeyCode::F7, KeyCode::F8, KeyCode::F9, KeyCode::F10, KeyCode::F11, KeyCode::F12,
        KeyCode::F13, KeyCode::F14, KeyCode::F15, KeyCode::F16, KeyCode::F17,
        KeyCode::F18, KeyCode::F19, KeyCode::F20, KeyCode::F21, KeyCode::F22,
        KeyCode::F23, KeyCode::F24,
        KeyCode::Escape, KeyCode::Enter, KeyCode::Tab, KeyCode::Space,
        KeyCode::Backspace, KeyCode::Delete, KeyCode::Insert,
        KeyCode::ArrowUp, KeyCode::ArrowDown, KeyCode::ArrowLeft, KeyCode::ArrowRight,
        KeyCode::Home, KeyCode::End, KeyCode::PageUp, KeyCode::PageDown,
        KeyCode::ShiftLeft, KeyCode::ShiftRight,
        KeyCode::ControlLeft, KeyCode::ControlRight,
        KeyCode::AltLeft, KeyCode::AltRight,
        KeyCode::SuperLeft, KeyCode::SuperRight, KeyCode::ContextMenu,
        KeyCode::Backquote, KeyCode::Minus, KeyCode::Equal,
        KeyCode::BracketLeft, KeyCode::BracketRight, KeyCode::Backslash,
        KeyCode::Semicolon, KeyCode::Quote, KeyCode::Comma, KeyCode::Period, KeyCode::Slash,
        KeyCode::CapsLock, KeyCode::NumLock, KeyCode::ScrollLock,
        KeyCode::PrintScreen, KeyCode::Pause,
        KeyCode::Numpad0, KeyCode::Numpad1, KeyCode::Numpad2, KeyCode::Numpad3,
        KeyCode::Numpad4, KeyCode::Numpad5, KeyCode::Numpad6, KeyCode::Numpad7,
        KeyCode::Numpad8, KeyCode::Numpad9,
        KeyCode::NumpadAdd, KeyCode::NumpadSubtract, KeyCode::NumpadMultiply,
        KeyCode::NumpadDivide, KeyCode::NumpadDecimal, KeyCode::NumpadEnter,
    ];

    /// 从名字解析键码。
    ///
    /// 同时接受三种写法，因为三种场合的"自然写法"不一样：
    /// - **变体名**：`"KeyW"`、`"ArrowUp"` —— 与 `input.ron` 一致，代码里用；
    /// - **显示名**：`"W"`、`"Up"`、`"Left Shift"` —— 与编辑器 UI 一致；
    /// - **大小写与分隔符随意**：`"w"` / `"left shift"` / `"leftshift"`。
    ///
    /// 大小写与空格都不敏感，是为了让脚本作者猜一次就能用对；
    /// 猜不中返回 `None`，由调用方决定是记警告还是静默 —— 引擎不替他们选。
    pub fn from_name(name: &str) -> Option<KeyCode> {
        let norm: String = name
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '_' && *c != '-')
            .flat_map(|c| c.to_lowercase())
            .collect();
        if norm.is_empty() {
            return None;
        }
        for &key in KeyCode::ALL {
            let variant = format!("{key:?}");
            let variant_norm: String = variant
                .chars()
                .filter(|c| !c.is_whitespace() && *c != '_')
                .flat_map(|c| c.to_lowercase())
                .collect();
            if variant_norm == norm {
                return Some(key);
            }
            let display_norm: String = key
                .display_name()
                .chars()
                .filter(|c| !c.is_whitespace())
                .flat_map(|c| c.to_lowercase())
                .collect();
            if display_norm == norm {
                return Some(key);
            }
        }
        None
    }
}

impl KeyCode {
    /// 这个键是不是修饰键。
    ///
    /// 编辑器与游戏都要在"录键绑定"时跳过修饰键（按 Ctrl 组合键时，
    /// 真正想绑的是组合后的结果，而不是把 Ctrl 本身录成动作）。
    pub fn is_modifier(self) -> bool {
        matches!(
            self,
            KeyCode::ShiftLeft
                | KeyCode::ShiftRight
                | KeyCode::ControlLeft
                | KeyCode::ControlRight
                | KeyCode::AltLeft
                | KeyCode::AltRight
                | KeyCode::SuperLeft
                | KeyCode::SuperRight
        )
    }

    /// 人类可读的名字，用于 UI 展示（编辑器绑定表、教程提示）。
    ///
    /// 与 `Debug` 不同：这里给的是玩家看得懂的 `"W"` / `"Left Shift"` / `"F1"`，
    /// 而不是变体名 `KeyW` / `ShiftLeft`。
    pub fn display_name(self) -> &'static str {
        match self {
            KeyCode::KeyA => "A", KeyCode::KeyB => "B", KeyCode::KeyC => "C",
            KeyCode::KeyD => "D", KeyCode::KeyE => "E", KeyCode::KeyF => "F",
            KeyCode::KeyG => "G", KeyCode::KeyH => "H", KeyCode::KeyI => "I",
            KeyCode::KeyJ => "J", KeyCode::KeyK => "K", KeyCode::KeyL => "L",
            KeyCode::KeyM => "M", KeyCode::KeyN => "N", KeyCode::KeyO => "O",
            KeyCode::KeyP => "P", KeyCode::KeyQ => "Q", KeyCode::KeyR => "R",
            KeyCode::KeyS => "S", KeyCode::KeyT => "T", KeyCode::KeyU => "U",
            KeyCode::KeyV => "V", KeyCode::KeyW => "W", KeyCode::KeyX => "X",
            KeyCode::KeyY => "Y", KeyCode::KeyZ => "Z",

            KeyCode::Digit0 => "0", KeyCode::Digit1 => "1", KeyCode::Digit2 => "2",
            KeyCode::Digit3 => "3", KeyCode::Digit4 => "4", KeyCode::Digit5 => "5",
            KeyCode::Digit6 => "6", KeyCode::Digit7 => "7", KeyCode::Digit8 => "8",
            KeyCode::Digit9 => "9",

            KeyCode::F1 => "F1", KeyCode::F2 => "F2", KeyCode::F3 => "F3",
            KeyCode::F4 => "F4", KeyCode::F5 => "F5", KeyCode::F6 => "F6",
            KeyCode::F7 => "F7", KeyCode::F8 => "F8", KeyCode::F9 => "F9",
            KeyCode::F10 => "F10", KeyCode::F11 => "F11", KeyCode::F12 => "F12",
            KeyCode::F13 => "F13", KeyCode::F14 => "F14", KeyCode::F15 => "F15",
            KeyCode::F16 => "F16", KeyCode::F17 => "F17", KeyCode::F18 => "F18",
            KeyCode::F19 => "F19", KeyCode::F20 => "F20", KeyCode::F21 => "F21",
            KeyCode::F22 => "F22", KeyCode::F23 => "F23", KeyCode::F24 => "F24",

            KeyCode::Escape => "Esc",
            KeyCode::Enter => "Enter",
            KeyCode::Tab => "Tab",
            KeyCode::Space => "Space",
            KeyCode::Backspace => "Backspace",
            KeyCode::Delete => "Delete",
            KeyCode::Insert => "Insert",

            KeyCode::ArrowUp => "Up",
            KeyCode::ArrowDown => "Down",
            KeyCode::ArrowLeft => "Left",
            KeyCode::ArrowRight => "Right",
            KeyCode::Home => "Home",
            KeyCode::End => "End",
            KeyCode::PageUp => "Page Up",
            KeyCode::PageDown => "Page Down",

            KeyCode::ShiftLeft => "Left Shift",
            KeyCode::ShiftRight => "Right Shift",
            KeyCode::ControlLeft => "Left Ctrl",
            KeyCode::ControlRight => "Right Ctrl",
            KeyCode::AltLeft => "Left Alt",
            KeyCode::AltRight => "Right Alt",
            KeyCode::SuperLeft => "Left Super",
            KeyCode::SuperRight => "Right Super",
            KeyCode::ContextMenu => "Menu",

            KeyCode::Backquote => "`",
            KeyCode::Minus => "-",
            KeyCode::Equal => "=",
            KeyCode::BracketLeft => "[",
            KeyCode::BracketRight => "]",
            KeyCode::Backslash => "\\",
            KeyCode::Semicolon => ";",
            KeyCode::Quote => "'",
            KeyCode::Comma => ",",
            KeyCode::Period => ".",
            KeyCode::Slash => "/",

            KeyCode::CapsLock => "Caps Lock",
            KeyCode::NumLock => "Num Lock",
            KeyCode::ScrollLock => "Scroll Lock",
            KeyCode::PrintScreen => "Print Screen",
            KeyCode::Pause => "Pause",

            KeyCode::Numpad0 => "Numpad 0", KeyCode::Numpad1 => "Numpad 1",
            KeyCode::Numpad2 => "Numpad 2", KeyCode::Numpad3 => "Numpad 3",
            KeyCode::Numpad4 => "Numpad 4", KeyCode::Numpad5 => "Numpad 5",
            KeyCode::Numpad6 => "Numpad 6", KeyCode::Numpad7 => "Numpad 7",
            KeyCode::Numpad8 => "Numpad 8", KeyCode::Numpad9 => "Numpad 9",
            KeyCode::NumpadAdd => "Numpad +",
            KeyCode::NumpadSubtract => "Numpad -",
            KeyCode::NumpadMultiply => "Numpad *",
            KeyCode::NumpadDivide => "Numpad /",
            KeyCode::NumpadDecimal => "Numpad .",
            KeyCode::NumpadEnter => "Numpad Enter",
        }
    }
}

/// 鼠标按键。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    /// 侧键（后退）。
    Back,
    /// 侧键（前进）。
    Forward,
    /// 其余按键（部分鼠标有 6 个以上）。
    Other(u16),
}

impl MouseButton {
    /// 从名字解析鼠标按键（大小写不敏感，接受 `"Left"` / `"left"` / `"Mouse Left"`）。
    pub fn from_name(name: &str) -> Option<MouseButton> {
        let norm: String = name
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '_' && *c != '-')
            .flat_map(|c| c.to_lowercase())
            .collect();
        match norm.as_str() {
            "left" | "mouseleft" | "lmb" | "0" => Some(MouseButton::Left),
            "right" | "mouseright" | "rmb" | "1" => Some(MouseButton::Right),
            "middle" | "mousemiddle" | "mmb" | "2" => Some(MouseButton::Middle),
            "back" | "mouseback" => Some(MouseButton::Back),
            "forward" | "mouseforward" => Some(MouseButton::Forward),
            _ => None,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            MouseButton::Left => "Mouse Left",
            MouseButton::Right => "Mouse Right",
            MouseButton::Middle => "Mouse Middle",
            MouseButton::Back => "Mouse Back",
            MouseButton::Forward => "Mouse Forward",
            MouseButton::Other(_) => "Mouse Other",
        }
    }
}

// ---------------------------------------------------------------------------
// winit 翻译层（仅 feature = "winit" 时编译）
// ---------------------------------------------------------------------------

#[cfg(feature = "winit")]
mod winit_conv {
    use super::{KeyCode, MouseButton};
    use winit::event::MouseButton as WinitMouseButton;
    use winit::keyboard::KeyCode as WinitKeyCode;

    /// 翻译 winit 键码；不支持的键返回 `None`，让调用方直接忽略。
    ///
    /// 刻意返回 `Option` 而不是实现 `From`：winit 里还有一堆媒体键、浏览器键、
    /// IME 键，它们和游戏输入无关。若用 `From` 就必须给这些键一个落点
    /// （退化成 `Escape` 之类的），结果是按一下音量键忽然"打开菜单"。
    /// 明确丢弃要诚实得多。
    pub fn key_from_winit(k: WinitKeyCode) -> Option<KeyCode> {
        use WinitKeyCode as W;
        Some(match k {
            W::KeyA => KeyCode::KeyA, W::KeyB => KeyCode::KeyB,
            W::KeyC => KeyCode::KeyC, W::KeyD => KeyCode::KeyD,
            W::KeyE => KeyCode::KeyE, W::KeyF => KeyCode::KeyF,
            W::KeyG => KeyCode::KeyG, W::KeyH => KeyCode::KeyH,
            W::KeyI => KeyCode::KeyI, W::KeyJ => KeyCode::KeyJ,
            W::KeyK => KeyCode::KeyK, W::KeyL => KeyCode::KeyL,
            W::KeyM => KeyCode::KeyM, W::KeyN => KeyCode::KeyN,
            W::KeyO => KeyCode::KeyO, W::KeyP => KeyCode::KeyP,
            W::KeyQ => KeyCode::KeyQ, W::KeyR => KeyCode::KeyR,
            W::KeyS => KeyCode::KeyS, W::KeyT => KeyCode::KeyT,
            W::KeyU => KeyCode::KeyU, W::KeyV => KeyCode::KeyV,
            W::KeyW => KeyCode::KeyW, W::KeyX => KeyCode::KeyX,
            W::KeyY => KeyCode::KeyY, W::KeyZ => KeyCode::KeyZ,

            W::Digit0 => KeyCode::Digit0, W::Digit1 => KeyCode::Digit1,
            W::Digit2 => KeyCode::Digit2, W::Digit3 => KeyCode::Digit3,
            W::Digit4 => KeyCode::Digit4, W::Digit5 => KeyCode::Digit5,
            W::Digit6 => KeyCode::Digit6, W::Digit7 => KeyCode::Digit7,
            W::Digit8 => KeyCode::Digit8, W::Digit9 => KeyCode::Digit9,

            W::F1 => KeyCode::F1, W::F2 => KeyCode::F2, W::F3 => KeyCode::F3,
            W::F4 => KeyCode::F4, W::F5 => KeyCode::F5, W::F6 => KeyCode::F6,
            W::F7 => KeyCode::F7, W::F8 => KeyCode::F8, W::F9 => KeyCode::F9,
            W::F10 => KeyCode::F10, W::F11 => KeyCode::F11, W::F12 => KeyCode::F12,
            W::F13 => KeyCode::F13, W::F14 => KeyCode::F14, W::F15 => KeyCode::F15,
            W::F16 => KeyCode::F16, W::F17 => KeyCode::F17, W::F18 => KeyCode::F18,
            W::F19 => KeyCode::F19, W::F20 => KeyCode::F20, W::F21 => KeyCode::F21,
            W::F22 => KeyCode::F22, W::F23 => KeyCode::F23, W::F24 => KeyCode::F24,

            W::Escape => KeyCode::Escape, W::Enter => KeyCode::Enter,
            W::Tab => KeyCode::Tab, W::Space => KeyCode::Space,
            W::Backspace => KeyCode::Backspace, W::Delete => KeyCode::Delete,
            W::Insert => KeyCode::Insert,

            W::ArrowUp => KeyCode::ArrowUp, W::ArrowDown => KeyCode::ArrowDown,
            W::ArrowLeft => KeyCode::ArrowLeft, W::ArrowRight => KeyCode::ArrowRight,
            W::Home => KeyCode::Home, W::End => KeyCode::End,
            W::PageUp => KeyCode::PageUp, W::PageDown => KeyCode::PageDown,

            W::ShiftLeft => KeyCode::ShiftLeft, W::ShiftRight => KeyCode::ShiftRight,
            W::ControlLeft => KeyCode::ControlLeft, W::ControlRight => KeyCode::ControlRight,
            W::AltLeft => KeyCode::AltLeft, W::AltRight => KeyCode::AltRight,
            W::SuperLeft => KeyCode::SuperLeft, W::SuperRight => KeyCode::SuperRight,
            W::ContextMenu => KeyCode::ContextMenu,

            W::Backquote => KeyCode::Backquote, W::Minus => KeyCode::Minus,
            W::Equal => KeyCode::Equal, W::BracketLeft => KeyCode::BracketLeft,
            W::BracketRight => KeyCode::BracketRight, W::Backslash => KeyCode::Backslash,
            W::Semicolon => KeyCode::Semicolon, W::Quote => KeyCode::Quote,
            W::Comma => KeyCode::Comma, W::Period => KeyCode::Period,
            W::Slash => KeyCode::Slash,

            W::CapsLock => KeyCode::CapsLock, W::NumLock => KeyCode::NumLock,
            W::ScrollLock => KeyCode::ScrollLock, W::PrintScreen => KeyCode::PrintScreen,
            W::Pause => KeyCode::Pause,

            W::Numpad0 => KeyCode::Numpad0, W::Numpad1 => KeyCode::Numpad1,
            W::Numpad2 => KeyCode::Numpad2, W::Numpad3 => KeyCode::Numpad3,
            W::Numpad4 => KeyCode::Numpad4, W::Numpad5 => KeyCode::Numpad5,
            W::Numpad6 => KeyCode::Numpad6, W::Numpad7 => KeyCode::Numpad7,
            W::Numpad8 => KeyCode::Numpad8, W::Numpad9 => KeyCode::Numpad9,
            W::NumpadAdd => KeyCode::NumpadAdd, W::NumpadSubtract => KeyCode::NumpadSubtract,
            W::NumpadMultiply => KeyCode::NumpadMultiply, W::NumpadDivide => KeyCode::NumpadDivide,
            W::NumpadDecimal => KeyCode::NumpadDecimal, W::NumpadEnter => KeyCode::NumpadEnter,

            _ => return None,
        })
    }

    impl From<WinitMouseButton> for MouseButton {
        fn from(b: WinitMouseButton) -> Self {
            match b {
                WinitMouseButton::Left => MouseButton::Left,
                WinitMouseButton::Right => MouseButton::Right,
                WinitMouseButton::Middle => MouseButton::Middle,
                WinitMouseButton::Back => MouseButton::Back,
                WinitMouseButton::Forward => MouseButton::Forward,
                WinitMouseButton::Other(n) => MouseButton::Other(n),
            }
        }
    }
}

#[cfg(feature = "winit")]
pub use winit_conv::key_from_winit;

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifier_detection() {
        assert!(KeyCode::ControlLeft.is_modifier());
        assert!(KeyCode::ShiftRight.is_modifier());
        assert!(!KeyCode::KeyW.is_modifier());
        assert!(!KeyCode::Space.is_modifier());
    }

    #[test]
    fn display_names_are_human_readable() {
        assert_eq!(KeyCode::KeyW.display_name(), "W");
        assert_eq!(KeyCode::Space.display_name(), "Space");
        assert_eq!(KeyCode::ShiftLeft.display_name(), "Left Shift");
    }

    #[test]
    fn keycode_serializes_by_variant_name() {
        let k = KeyCode::KeyW;
        let s = ron::to_string(&k).unwrap();
        assert!(s.contains("KeyW"), "序列化结果应保留可读的变体名, got {s}");
    }
}
