//! 输入映射：解析项目 `input.ron`，把按键 / 鼠标状态折叠成动作与轴。

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum KeyCode {
    Space,
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    ShiftLeft,
    ShiftRight,
    ControlLeft,
    ControlRight,
    AltLeft,
    AltRight,
    KeyA,
    KeyB,
    KeyC,
    KeyD,
    KeyE,
    KeyF,
    KeyG,
    KeyH,
    KeyI,
    KeyJ,
    KeyK,
    KeyL,
    KeyM,
    KeyN,
    KeyO,
    KeyP,
    KeyQ,
    KeyR,
    KeyS,
    KeyT,
    KeyU,
    KeyV,
    KeyW,
    KeyX,
    KeyY,
    KeyZ,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Combo {
    pub key: KeyCode,
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub control: bool,
    #[serde(default)]
    pub alt: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub enum Binding {
    Key(KeyCode),
    Mouse(MouseButton),
    Combo(Combo),
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Axis {
    #[serde(default)]
    pub positive: Vec<Binding>,
    #[serde(default)]
    pub negative: Vec<Binding>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct InputMap {
    #[serde(default)]
    pub actions: BTreeMap<String, Vec<Binding>>,
    #[serde(default)]
    pub axes: BTreeMap<String, Axis>,
}

#[derive(Clone, Debug, Default)]
pub struct InputState {
    pub keys: BTreeSet<KeyCode>,
    pub mouse: BTreeSet<MouseButton>,
}

impl InputMap {
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        std::fs::read_to_string(path)
            .map_err(|err| format!("读取输入映射失败：{err}"))
            .and_then(|text| Self::parse_ron(&text))
            .map_err(|err| format!("{}：{err}", path.display()))
    }

    pub fn parse_ron(text: &str) -> Result<Self, String> {
        ron::from_str(text).map_err(|err| format!("输入映射解析失败：{err}"))
    }

    pub fn action_names(&self) -> Vec<String> {
        self.actions.keys().cloned().collect()
    }

    pub fn axis_names(&self) -> Vec<String> {
        self.axes.keys().cloned().collect()
    }

    pub fn action_pressed(&self, name: &str, state: &InputState) -> bool {
        self.actions
            .get(name)
            .is_some_and(|bindings| bindings.iter().any(|binding| pressed(binding, state)))
    }

    /// 正负都按下时归零，避免轴抖动。
    pub fn axis_value(&self, name: &str, state: &InputState) -> f32 {
        let Some(axis) = self.axes.get(name) else {
            return 0.0;
        };
        let positive = axis.positive.iter().any(|binding| pressed(binding, state));
        let negative = axis.negative.iter().any(|binding| pressed(binding, state));
        match (positive, negative) {
            (true, false) => 1.0,
            (false, true) => -1.0,
            _ => 0.0,
        }
    }
}

fn pressed(binding: &Binding, state: &InputState) -> bool {
    match binding {
        Binding::Key(key) => state.keys.contains(key),
        Binding::Mouse(button) => state.mouse.contains(button),
        Binding::Combo(combo) => {
            state.keys.contains(&combo.key)
                && combo.shift
                    == (state.keys.contains(&KeyCode::ShiftLeft)
                        || state.keys.contains(&KeyCode::ShiftRight))
                && combo.control
                    == (state.keys.contains(&KeyCode::ControlLeft)
                        || state.keys.contains(&KeyCode::ControlRight))
                && combo.alt
                    == (state.keys.contains(&KeyCode::AltLeft)
                        || state.keys.contains(&KeyCode::AltRight))
        }
    }
}
