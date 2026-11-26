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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputState {
    pub keys: BTreeSet<KeyCode>,
    pub mouse: BTreeSet<MouseButton>,
}

#[derive(Clone, Copy, Debug)]
pub enum InputEdge {
    Key {
        code: KeyCode,
        pressed: bool,
        /// 记录主键边沿时的 Shift、Control、Alt，短按完成后仍可匹配组合键。
        modifiers: [bool; 3],
    },
    Mouse {
        button: MouseButton,
        pressed: bool,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActionInput {
    pub down: bool,
    pub pressed: bool,
    pub released: bool,
}

impl InputState {
    /// 无窗口调用方提供的是状态快照，从连续采样中提取可观察到的边沿。
    pub fn append_edges_since(&self, previous: &Self, edges: &mut Vec<InputEdge>) {
        let modifiers = self.modifiers();
        for (current, other, pressed) in [(self, previous, true), (previous, self, false)] {
            edges.extend(
                current
                    .keys
                    .difference(&other.keys)
                    .map(|&code| InputEdge::Key {
                        code,
                        pressed,
                        modifiers,
                    }),
            );
            edges.extend(
                current
                    .mouse
                    .difference(&other.mouse)
                    .map(|&button| InputEdge::Mouse { button, pressed }),
            );
        }
    }

    fn modifiers(&self) -> [bool; 3] {
        [
            [KeyCode::ShiftLeft, KeyCode::ShiftRight],
            [KeyCode::ControlLeft, KeyCode::ControlRight],
            [KeyCode::AltLeft, KeyCode::AltRight],
        ]
        .map(|keys| keys.iter().any(|key| self.keys.contains(key)))
    }
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

    pub fn action_down(&self, name: &str, state: &InputState) -> bool {
        self.actions
            .get(name)
            .is_some_and(|bindings| bindings.iter().any(|binding| held(binding, state)))
    }

    pub fn action_input(&self, name: &str, state: &InputState, edges: &[InputEdge]) -> ActionInput {
        let mut action = ActionInput {
            down: self.action_down(name, state),
            ..ActionInput::default()
        };
        let Some(bindings) = self.actions.get(name) else {
            return action;
        };
        // 任一绑定的边沿都算，即使同一动作的另一绑定已经按住。
        for edge in edges.iter().copied() {
            if bindings.iter().any(|binding| matches_edge(binding, edge)) {
                match edge {
                    InputEdge::Key { pressed, .. } | InputEdge::Mouse { pressed, .. } => {
                        action.pressed |= pressed;
                        action.released |= !pressed;
                    }
                }
            }
        }
        action
    }

    /// 正负都按下时归零，避免轴抖动。
    pub fn axis_value(&self, name: &str, state: &InputState) -> f32 {
        let Some(axis) = self.axes.get(name) else {
            return 0.0;
        };
        let positive = axis.positive.iter().any(|binding| held(binding, state));
        let negative = axis.negative.iter().any(|binding| held(binding, state));
        match (positive, negative) {
            (true, false) => 1.0,
            (false, true) => -1.0,
            _ => 0.0,
        }
    }
}

impl Combo {
    fn matches(&self, modifiers: [bool; 3]) -> bool {
        [self.shift, self.control, self.alt] == modifiers
    }
}

fn held(binding: &Binding, state: &InputState) -> bool {
    match binding {
        Binding::Key(key) => state.keys.contains(key),
        Binding::Mouse(button) => state.mouse.contains(button),
        Binding::Combo(combo) => {
            state.keys.contains(&combo.key) && combo.matches(state.modifiers())
        }
    }
}

fn matches_edge(binding: &Binding, edge: InputEdge) -> bool {
    match (binding, edge) {
        (Binding::Key(key), InputEdge::Key { code, .. }) => *key == code,
        (Binding::Mouse(expected), InputEdge::Mouse { button, .. }) => *expected == button,
        (
            Binding::Combo(combo),
            InputEdge::Key {
                code,
                pressed,
                modifiers,
            },
        ) => {
            // 松开主键时修饰键可能已经先松开，仍须报告释放。
            combo.key == code && (!pressed || combo.matches(modifiers))
        }
        _ => false,
    }
}
