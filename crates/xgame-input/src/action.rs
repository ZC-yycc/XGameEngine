//! 动作映射：把物理按键翻译成**语义动作**。
//!
//! 游戏逻辑里写 `input.action_down("Jump")`，而不是 `input.key_down(KeyCode::Space)`。
//! 这一步间接带来的三件好事，正是商业引擎（Unity Input System、UE Enhanced Input）
//! 都在做的事：
//!
//! 1. **可重绑定**：玩家改按键不用重编游戏，改的是 `input.ron`；
//! 2. **多对一**：跳跃可以同时绑 `Space` 和小键盘 `Numpad0`，不用写两个 `if`；
//! 3. **轴抽象**：`MoveX` 是一条 -1..1 的轴，绑 `A`/`D` 只是它的默认实现 ——
//!    换成手柄摇杆、或者接到 AI 产出的一条轴上，游戏代码一行都不用改。
//!
//! # 文件格式（项目根目录的 `input.ron`）
//!
//! ```ron
//! InputMap(
//!     actions: {
//!         "Jump": [Key(Space)],
//!         "Fire": [Mouse(Left), Key(ControlLeft)],
//!     },
//!     axes: {
//!         "MoveX": (positive: [Key(KeyD)], negative: [Key(KeyA)]),
//!         "MoveY": (positive: [Key(KeyW)], negative: [Key(KeyS)]),
//!     },
//! )
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::key::{KeyCode, MouseButton};
use crate::pad::{apply_deadzone, GamepadAxis, GamepadButton};
use crate::state::InputState;

/// 一条按键绑定。
///
/// 用 `BTreeMap` 而非 `HashMap` 承载动作表，是为了**序列化结果稳定**：
/// 同一个 `InputMap` 存两次得到字节相同的文件，diff 才干净、才适合进版本库。
// 注意：`PadAxis` 带一个 `f32` 阈值，所以这里不能派生 `Eq` / `Hash`。
// 需要"两条绑定是否相同"的地方用 `PartialEq`（阈值按位比较，够用）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Binding {
    /// 单个键（忽略修饰键状态）。
    Key(KeyCode),
    /// 单个鼠标按键。
    Mouse(MouseButton),
    /// 「修饰键 + 主键」的组合，例如 `Ctrl+S`。
    ///
    /// 四个修饰位是**精确匹配**：`ctrl: true` 意味着"按住了 Ctrl，且没按 Shift/Alt/Super"。
    /// 精确匹配是刻意的 —— 否则 `Ctrl+S` 会在玩家顺手按着 Shift 时突然失效，
    /// 而且 `S` 与 `Ctrl+S` 会互相抢触发。
    Combo {
        key: KeyCode,
        #[serde(default)]
        ctrl: bool,
        #[serde(default)]
        shift: bool,
        #[serde(default)]
        alt: bool,
        #[serde(default)]
        sup: bool,
    },
    /// 手柄上的一个语义按键（`PadButton(South)`）。
    PadButton(GamepadButton),
    /// 手柄的一根轴当按钮用：**越过阈值**就算按下（扳机最典型的用法）。
    ///
    /// 写成轴而不是"扳机专用按键"：扳机在部分后端只上报轴、不按钮化，
    /// 而且"按到一半算不算按下"是个**手感**参数，应该由作者定。
    PadAxis {
        axis: GamepadAxis,
        /// 触发阈值（0..1）。低于它的按压不算按下。
        #[serde(default = "default_pad_threshold")]
        threshold: f32,
    },
}

/// 轴当按钮用时的默认阈值。
///
/// 0.5 与各家驱动的"数字化扳机"一致；调低（0.2）会让轻碰就开火，
/// 调高（0.8）适合"扣到底才蓄力"的设计。
fn default_pad_threshold() -> f32 {
    0.5
}

impl Binding {
    /// `Ctrl+键`。
    pub fn ctrl(key: KeyCode) -> Self {
        Binding::Combo {
            key,
            ctrl: true,
            shift: false,
            alt: false,
            sup: false,
        }
    }

    /// `Shift+键`。
    pub fn shift(key: KeyCode) -> Self {
        Binding::Combo {
            key,
            ctrl: false,
            shift: true,
            alt: false,
            sup: false,
        }
    }

    /// UI 上显示的名字，例如 `"Ctrl+S"`。
    pub fn display_name(&self) -> String {
        match self {
            Binding::Key(k) => k.display_name().to_owned(),
            Binding::Mouse(m) => m.display_name().to_owned(),
            Binding::PadButton(button) => button.display_name().to_owned(),
            Binding::PadAxis { axis, threshold } => {
                format!("{} > {threshold:.2}", axis.display_name())
            }
            Binding::Combo {
                key,
                ctrl,
                shift,
                alt,
                sup,
            } => {
                let mut parts: Vec<&str> = Vec::new();
                if *ctrl {
                    parts.push("Ctrl");
                }
                if *shift {
                    parts.push("Shift");
                }
                if *alt {
                    parts.push("Alt");
                }
                if *sup {
                    parts.push("Super");
                }
                parts.push(key.display_name());
                parts.join("+")
            }
        }
    }

    /// 这条绑定是不是手柄绑定（"项目自己表达过手柄意见"的判据）。
    pub fn is_pad(&self) -> bool {
        matches!(self, Binding::PadButton(_) | Binding::PadAxis { .. })
    }

    /// 这一刻绑定是否成立。
    fn is_down(&self, state: &InputState) -> bool {
        match self {
            Binding::Key(k) => state.key_down(*k),
            Binding::Mouse(m) => state.mouse_down(*m),
            Binding::Combo {
                key,
                ctrl,
                shift,
                alt,
                sup,
            } => state.key_down(*key) && state.modifiers_match(*ctrl, *shift, *alt, *sup),
            Binding::PadButton(button) => state.pad_button_down(*button),
            Binding::PadAxis { axis, threshold } => state.pad_axis(*axis) >= *threshold,
        }
    }

    /// 本帧是否刚触发。
    fn is_pressed(&self, state: &InputState) -> bool {
        match self {
            Binding::Key(k) => state.key_pressed(*k),
            Binding::Mouse(m) => state.mouse_pressed(*m),
            // 组合键只认"主键刚按下"这一瞬间，并要求修饰键当时是精确的那一组。
            // 反过来（修饰键刚按下就算触发）会让 `Ctrl+S` 在你先按 Ctrl 时
            // 毫无预兆地立刻触发一次。
            Binding::Combo {
                key,
                ctrl,
                shift,
                alt,
                sup,
            } => state.key_pressed(*key) && state.modifiers_match(*ctrl, *shift, *alt, *sup),
            Binding::PadButton(button) => state.pad_button_pressed(*button),
            // 轴没有"按键事件"，边沿要靠上一帧的值比出来（见 `PadState::axis_crossed_up`）。
            Binding::PadAxis { axis, threshold } => state.pad_axis_crossed_up(*axis, *threshold),
        }
    }

    /// 本帧是否刚松开。
    fn is_released(&self, state: &InputState) -> bool {
        match self {
            Binding::Key(k) => state.key_released(*k),
            Binding::Mouse(m) => state.mouse_released(*m),
            Binding::Combo { key, .. } => state.key_released(*key),
            Binding::PadButton(button) => state.pad_button_released(*button),
            Binding::PadAxis { axis, threshold } => state.pad_axis_crossed_down(*axis, *threshold),
        }
    }
}

/// 一条双向轴（例如 `MoveX = D - A`）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AxisBinding {
    /// 正方向按键（+1）。
    #[serde(default)]
    pub positive: Vec<Binding>,
    /// 负方向按键（-1）。
    #[serde(default)]
    pub negative: Vec<Binding>,
    /// 模拟量来源（摇杆）：直接给出 `-1..1` 的连续值，而不是 ±1 的台阶。
    ///
    /// 与按键**相加**再夹到 `-1..1`：键盘推满 + 摇杆推满等于推满，不会溢出；
    /// 只想用摇杆时不写 `positive`/`negative` 即可。
    ///
    /// 空表不落盘（老项目保存后字节不变）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub analog: Vec<AnalogBinding>,
}

/// 一条模拟轴绑定（摇杆 → 轴值）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnalogBinding {
    /// 取自哪根手柄轴。
    pub axis: GamepadAxis,
    /// 缩放系数（负数用于反向，例如"推左 = 右转"）。
    #[serde(default = "one")]
    pub scale: f32,
    /// 死区（0..1）：绝对值小于它的输入压成 0，其余重标定到满量程。
    #[serde(default = "default_pad_deadzone")]
    pub deadzone: f32,
}

fn one() -> f32 {
    1.0
}

/// 摇杆默认死区。
///
/// 0.15 是各家的常见值（Unity 默认 0.19、Godot 轴向 0.5 但那是"轴向死区"）：
/// 廉价手柄的摇杆回中误差常到 0.08，低于这个数就开始"自己走"。
fn default_pad_deadzone() -> f32 {
    0.15
}

impl AnalogBinding {
    /// 取这根轴**应用死区与缩放之后**的值。
    pub fn value(&self, state: &InputState) -> f32 {
        let raw = state.pad_axis(self.axis);
        // 扳机是单极（0..1），死区重标定之后仍然是 0..1；摇杆是双极。
        apply_deadzone(raw, self.deadzone) * self.scale
    }
}

/// 动作 → 绑定 的映射表。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InputMap {
    /// 离散动作（按下 / 按住 / 松开）。
    #[serde(default)]
    pub actions: BTreeMap<String, Vec<Binding>>,
    /// 连续轴。
    #[serde(default)]
    pub axes: BTreeMap<String, AxisBinding>,
}

/// 读写输入映射出错的地方。
#[derive(Debug)]
pub enum InputMapError {
    Io(std::io::Error),
    Parse(String),
}

impl std::fmt::Display for InputMapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InputMapError::Io(e) => write!(f, "IO 错误: {e}"),
            InputMapError::Parse(e) => write!(f, "解析错误: {e}"),
        }
    }
}

impl std::error::Error for InputMapError {}

impl InputMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// 绑定一个离散动作（追加，不影响已有绑定 —— 多对一是特性不是意外）。
    pub fn bind(&mut self, action: impl Into<String>, binding: Binding) -> &mut Self {
        self.actions.entry(action.into()).or_default().push(binding);
        self
    }

    /// 设置一条轴的正负绑定。
    pub fn bind_axis(
        &mut self,
        axis: impl Into<String>,
        positive: Vec<Binding>,
        negative: Vec<Binding>,
    ) -> &mut Self {
        self.axes.insert(
            axis.into(),
            AxisBinding {
                positive,
                negative,
                analog: Vec::new(),
            },
        );
        self
    }

    pub fn action_names(&self) -> impl Iterator<Item = &str> {
        self.actions.keys().map(String::as_str)
    }

    pub fn axis_names(&self) -> impl Iterator<Item = &str> {
        self.axes.keys().map(String::as_str)
    }

    /// 默认的第一人称游戏映射（`WASD` + 鼠标视角 + 跳跃 + 开火……）。
    ///
    /// 之所以给一份默认表而不是空表：新建的项目立刻就能跑 `WASD` 走路，
    /// 而不是先对着空白的 `input.ron` 发愣。
    pub fn default_game() -> Self {
        let mut map = Self::new();

        map.bind("Jump", Binding::Key(KeyCode::Space));
        map.bind("Fire", Binding::Mouse(MouseButton::Left));
        map.bind("Fire", Binding::Key(KeyCode::ControlLeft));
        map.bind("Aim", Binding::Mouse(MouseButton::Right));
        map.bind("Sprint", Binding::Key(KeyCode::ShiftLeft));
        map.bind("Crouch", Binding::Key(KeyCode::ControlLeft));
        map.bind("Interact", Binding::Key(KeyCode::KeyE));
        map.bind("Reload", Binding::Key(KeyCode::KeyR));
        // 用 P 而不是 Esc：独立游戏里 Esc 已经被"退出"占用了，
        // 再把它绑成暂停会让玩家按一次就退出游戏。
        map.bind("TogglePause", Binding::Key(KeyCode::KeyP));
        // 数字键快速切换 —— 多对一：物理位置 Digit1..Digit5 是绝大多数玩家的直觉。
        for (i, code) in [
            KeyCode::Digit1,
            KeyCode::Digit2,
            KeyCode::Digit3,
            KeyCode::Digit4,
            KeyCode::Digit5,
        ]
        .into_iter()
        .enumerate()
        {
            map.bind(format!("Slot{}", i + 1), Binding::Key(code));
        }

        map.bind_axis(
            "MoveX",
            vec![
                Binding::Key(KeyCode::KeyD),
                Binding::Key(KeyCode::ArrowRight),
            ],
            vec![
                Binding::Key(KeyCode::KeyA),
                Binding::Key(KeyCode::ArrowLeft),
            ],
        );
        map.bind_axis(
            "MoveY",
            vec![Binding::Key(KeyCode::KeyW), Binding::Key(KeyCode::ArrowUp)],
            vec![
                Binding::Key(KeyCode::KeyS),
                Binding::Key(KeyCode::ArrowDown),
            ],
        );
        // 上下轴单独一条：`MoveY` 是"水平方向的前后"，`Lift` 是垂直升降。
        map.bind_axis(
            "Lift",
            vec![Binding::Key(KeyCode::KeyE)],
            vec![Binding::Key(KeyCode::KeyQ)],
        );
        // 鼠标视角轴：正方向用鼠标**移动**而不是按键，是 FPS 的惯例。
        // 这里用 `Mouse` 绑定表达不了"鼠标位移"，所以视角轴由相机代码直接读
        // `InputState::mouse_delta`，不进这张表 —— 见下面的 `look_delta`。

        // 手柄：与键盘**共存**（多对一），插上手柄就能走，不用先删键位。
        map.apply_pad_defaults();

        map
    }

    /// 这张表里是否已经有任何**手柄**绑定。
    pub fn has_pad_bindings(&self) -> bool {
        self.actions.values().flatten().any(Binding::is_pad)
            || self.axes.values().any(|a| {
                !a.analog.is_empty()
                    || a.positive.iter().chain(a.negative.iter()).any(Binding::is_pad)
            })
    }

    /// 补上**默认手柄绑定**（只增不改：已有的绑定一条都不动）。
    ///
    /// 动作名沿用 [`Self::default_game`] 那套（`Jump` / `Fire` / …）：自定义表里
    /// 没有这些名字时它们会被创建，多出来的动作对游戏没有影响（没人读就不会
    /// 触发任何逻辑），但玩家插上手柄就有反应。
    ///
    /// 轴只**追加**模拟源，不覆盖作者写的按键。
    pub fn apply_pad_defaults(&mut self) {
        use GamepadAxis as A;
        use GamepadButton as B;

        self.bind("Jump", Binding::PadButton(B::South));
        self.bind("Interact", Binding::PadButton(B::West));
        // 扳机当按钮：阈值 0.4 比驱动的"数字化"阈值略低，扣到一半就开火，
        // 手感上更像"轻按即射"；要蓄力就把阈值调高。
        self.bind(
            "Fire",
            Binding::PadAxis {
                axis: A::RightTrigger,
                threshold: 0.4,
            },
        );
        self.bind(
            "Aim",
            Binding::PadAxis {
                axis: A::LeftTrigger,
                threshold: 0.4,
            },
        );
        self.bind("Sprint", Binding::PadButton(B::LeftStick));
        self.bind("TogglePause", Binding::PadButton(B::Start));

        // 左摇杆 = 移动，右摇杆 = 视角。`+Y` 在上（与 `W` 的正方向一致）。
        self.axes.entry("MoveX".into()).or_default().analog.push(
            AnalogBinding { axis: A::LeftX, scale: 1.0, deadzone: default_pad_deadzone() },
        );
        self.axes.entry("MoveY".into()).or_default().analog.push(
            AnalogBinding { axis: A::LeftY, scale: 1.0, deadzone: default_pad_deadzone() },
        );
        self.axes.entry("LookX".into()).or_default().analog.push(
            AnalogBinding { axis: A::RightX, scale: 1.0, deadzone: default_pad_deadzone() },
        );
        self.axes.entry("LookY".into()).or_default().analog.push(
            AnalogBinding { axis: A::RightY, scale: 1.0, deadzone: default_pad_deadzone() },
        );
    }

    /// 老项目（写 `input.ron` 时还没有手柄支持）加载后自动补默认手柄绑定。
    ///
    /// 判据是"**这张表里一条手柄绑定都没有**"：作者只要动过手柄绑定（哪怕
    /// 只是删掉一条），就说明他表达过意见，之后不再自动补 —— 否则玩家改的
    /// 键位会被下次启动悄悄恢复。这条规则只在宿主的加载路径上生效，
    /// `load` 本身是纯粹的反序列化。
    pub fn with_pad_defaults_if_absent(mut self) -> Self {
        if !self.has_pad_bindings() {
            self.apply_pad_defaults();
        }
        self
    }

    /// 从 RON 文件加载。文件不存在时返回 `Ok(None)`（调用方退回默认映射），
    /// 解析失败才返回 `Err` —— 「还没有这个文件」和「这个文件写坏了」
    /// 是两件完全不同的事，不该用同一个错误吞掉。
    pub fn load(path: &Path) -> Result<Option<Self>, InputMapError> {
        if !path.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(path).map_err(InputMapError::Io)?;
        ron::from_str(&text)
            .map(Some)
            .map_err(|e| InputMapError::Parse(e.to_string()))
    }

    /// 存到 RON 文件（`pretty` 输出，方便手改与 diff）。
    ///
    /// 落盘走 `xgame_core::fsx::write_atomic`（同目录独占暂存 + 同步 + 替换）：
    /// 半截的 `input.ron` 会让整个项目的按键在下次启动时退回默认映射，
    /// 比"保存失败"糟得多。
    pub fn save(&self, path: &Path) -> Result<(), InputMapError> {
        let ron = ron::ser::PrettyConfig::new()
            .depth_limit(4)
            .enumerate_arrays(true);
        let text = ron::ser::to_string_pretty(self, ron)
            .map_err(|e| InputMapError::Parse(e.to_string()))?;
        xgame_core::fsx::write_atomic(path, text).map_err(InputMapError::Io)
    }
}

/// 输入门面：设备状态 + 动作映射。
///
/// 游戏与编辑器都只跟这个类型打交道。它同时是"资源"（映射表可热改）
/// 和"每帧状态"（键鼠快照），所以两层都收在里面。
#[derive(Debug, Clone)]
pub struct Input {
    state: InputState,
    map: InputMap,
}

impl Input {
    pub fn new(map: InputMap) -> Self {
        Self {
            state: InputState::new(),
            map,
        }
    }

    /// 使用默认游戏映射。
    pub fn with_default_map() -> Self {
        Self::new(InputMap::default_game())
    }

    pub fn state(&self) -> &InputState {
        &self.state
    }

    /// 可变取用设备状态（平台层注入事件用）。
    pub fn state_mut(&mut self) -> &mut InputState {
        &mut self.state
    }

    pub fn map(&self) -> &InputMap {
        &self.map
    }

    /// 换一份映射（运行时重绑定 / 加载 `input.ron` 之后调用）。
    pub fn set_map(&mut self, map: InputMap) {
        self.map = map;
    }

    /// 帧末清理：清掉"仅本帧有效"的按键边沿与位移。
    pub fn end_frame(&mut self) {
        self.state.end_frame();
    }

    /// 重置全部状态（场景切换）。
    pub fn reset(&mut self) {
        self.state.reset();
    }

    // -----------------------------------------------------------------------
    // 动作查询
    // -----------------------------------------------------------------------

    /// 动作是否被按住（任意一条绑定成立即可）。
    pub fn action_down(&self, action: &str) -> bool {
        self.map
            .actions
            .get(action)
            .is_some_and(|binds| binds.iter().any(|b| b.is_down(&self.state)))
    }

    /// 动作本帧是否刚触发。
    pub fn action_pressed(&self, action: &str) -> bool {
        self.map
            .actions
            .get(action)
            .is_some_and(|binds| binds.iter().any(|b| b.is_pressed(&self.state)))
    }

    /// 动作本帧是否刚松开。
    pub fn action_released(&self, action: &str) -> bool {
        self.map
            .actions
            .get(action)
            .is_some_and(|binds| binds.iter().any(|b| b.is_released(&self.state)))
    }

    /// 轴的当前值，落在 `-1.0 ..= 1.0`。
    ///
    /// 正负同时按住时**互相抵消回 0** —— 而不是"后按的赢"。
    /// 抵消是更可预测的行为：玩家按住 `W` 再补一个 `S` 应该是停下，
    /// 而不是取决于两个键的按下顺序。
    pub fn axis(&self, axis: &str) -> f32 {
        let Some(binding) = self.map.axes.get(axis) else {
            return 0.0;
        };
        let pos = binding.positive.iter().any(|b| b.is_down(&self.state)) as i32;
        let neg = binding.negative.iter().any(|b| b.is_down(&self.state)) as i32;
        // 按键给 ±1 的台阶，摇杆给连续值，两者**相加再夹住**：
        // 谁在用哪套设备都不必区分，混用也不会越界（键盘推满 + 摇杆推满 = 1.0）。
        let digital = (pos - neg) as f32;
        let analog: f32 = binding.analog.iter().map(|a| a.value(&self.state)).sum();
        (digital + analog).clamp(-1.0, 1.0)
    }

    /// 二维轴组合。
    pub fn axis2(&self, x: &str, y: &str) -> xgame_math::glam::Vec2 {
        xgame_math::glam::Vec2::new(self.axis(x), self.axis(y))
    }

    // -----------------------------------------------------------------------
    // 直通设备查询（不需要映射的场景：编辑器的相机控制、录键界面）
    // -----------------------------------------------------------------------

    pub fn key_down(&self, code: KeyCode) -> bool {
        self.state.key_down(code)
    }

    pub fn key_pressed(&self, code: KeyCode) -> bool {
        self.state.key_pressed(code)
    }

    pub fn mouse_down(&self, button: MouseButton) -> bool {
        self.state.mouse_down(button)
    }

    pub fn mouse_pressed(&self, button: MouseButton) -> bool {
        self.state.mouse_pressed(button)
    }

    pub fn mouse_pos(&self) -> xgame_math::glam::Vec2 {
        self.state.mouse_pos()
    }

    pub fn mouse_delta(&self) -> xgame_math::glam::Vec2 {
        self.state.mouse_delta()
    }

    pub fn scroll(&self) -> xgame_math::glam::Vec2 {
        self.state.scroll()
    }

    // -----------------------------------------------------------------------
    // 手柄直通（不需要映射的场景：UI 导航、调试面板、脚本 API）
    // -----------------------------------------------------------------------

    /// 当前接了几台手柄。
    pub fn pad_count(&self) -> usize {
        self.state.pad_count()
    }

    /// 有没有任何一台手柄在连着。
    pub fn pad_connected(&self) -> bool {
        self.state.pad_count() > 0
    }

    pub fn pad_button_down(&self, button: GamepadButton) -> bool {
        self.state.pad_button_down(button)
    }

    pub fn pad_button_pressed(&self, button: GamepadButton) -> bool {
        self.state.pad_button_pressed(button)
    }

    pub fn pad_axis(&self, axis: GamepadAxis) -> f32 {
        self.state.pad_axis(axis)
    }

    /// 视角位移（鼠标移动 + 右摇杆类的输入），FPS 相机直接消费。
    ///
    /// 单独开一个方法，是因为"鼠标位移"没法表达成按键绑定，
    /// 硬塞进 `axes` 表反而会让人以为它可重绑定。
    pub fn look_delta(&self) -> xgame_math::glam::Vec2 {
        self.state.mouse_delta()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(input: &mut Input, code: KeyCode) {
        input.state_mut().handle_key(code, true);
    }

    #[test]
    fn action_maps_multiple_bindings() {
        let mut input = Input::with_default_map();
        press(&mut input, KeyCode::Space);
        assert!(input.action_pressed("Jump"));
        input.end_frame();
        assert!(!input.action_pressed("Jump"));
        assert!(input.action_down("Jump"));
    }

    #[test]
    fn axis_is_differential() {
        let mut input = Input::with_default_map();
        assert_eq!(input.axis("MoveX"), 0.0);
        press(&mut input, KeyCode::KeyD);
        assert_eq!(input.axis("MoveX"), 1.0);
        press(&mut input, KeyCode::KeyA);
        assert_eq!(input.axis("MoveX"), 0.0, "正负同时按住应抵消为 0");
    }

    #[test]
    fn combo_requires_exact_modifiers() {
        let mut map = InputMap::new();
        map.bind("Save", Binding::ctrl(KeyCode::KeyS));
        let mut input = Input::new(map);

        press(&mut input, KeyCode::ControlLeft);
        press(&mut input, KeyCode::KeyS);
        assert!(input.action_pressed("Save"));

        // 再加一个 Shift：修饰键不再精确匹配，绑定应当失效。
        press(&mut input, KeyCode::ShiftLeft);
        assert!(!input.action_pressed("Save"));
    }

    #[cfg(windows)]
    #[test]
    fn save_rejects_locked_target_without_changing_old_map() {
        use std::os::windows::fs::OpenOptionsExt;
        let path = std::env::temp_dir().join(format!(
            "xgame_input_save_lock_{}_{}.ron",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut old = InputMap::new();
        old.bind("Old", Binding::Key(KeyCode::KeyA));
        old.save(&path).unwrap();
        let before = std::fs::read(&path).unwrap();
        // 旧版本的固定暂存残留也不得阻断保存，更不能被这次失败清掉。
        let legacy_tmp = path.with_extension("ron.tmp");
        std::fs::write(&legacy_tmp, b"incomplete previous save").unwrap();
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(&path)
            .unwrap();
        let mut new = InputMap::new();
        new.bind("New", Binding::Key(KeyCode::KeyB));
        assert!(new.save(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(
            std::fs::read(&legacy_tmp).unwrap(),
            b"incomplete previous save"
        );
        drop(lock);
        new.save(&path).unwrap();
        assert!(InputMap::load(&path)
            .unwrap()
            .unwrap()
            .actions
            .contains_key("New"));
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(legacy_tmp).unwrap();
    }

    #[test]
    fn pad_button_binding_triggers_the_action() {
        let mut input = Input::with_default_map();
        assert!(!input.action_down("Jump"));
        input.state_mut().handle_pad_button(0, GamepadButton::South, true);
        assert!(input.action_pressed("Jump"), "默认映射里 South = 跳");
        input.end_frame();
        assert!(input.action_down("Jump"));
        assert!(!input.action_pressed("Jump"), "边沿只活一帧");
        input.state_mut().handle_pad_button(0, GamepadButton::South, false);
        assert!(input.action_released("Jump"));
        assert!(!input.action_down("Jump"));
    }

    #[test]
    fn trigger_axis_is_thresholded_only_on_crossing() {
        let mut input = Input::with_default_map();
        // 默认 Fire 绑右扳机，阈值 0.4。
        input.state_mut().handle_pad_axis(0, GamepadAxis::RightTrigger, 0.2);
        assert!(!input.action_down("Fire"), "没到阈值不算按下");
        input.end_frame();

        input.state_mut().handle_pad_axis(0, GamepadAxis::RightTrigger, 0.5);
        assert!(input.action_down("Fire"));
        assert!(input.action_pressed("Fire"), "越过阈值的那一帧是边沿");
        input.end_frame();

        // 继续加深不该重复触发。
        input.state_mut().handle_pad_axis(0, GamepadAxis::RightTrigger, 0.9);
        assert!(input.action_down("Fire"));
        assert!(!input.action_pressed("Fire"));

        input.state_mut().handle_pad_axis(0, GamepadAxis::RightTrigger, 0.1);
        assert!(input.action_released("Fire"));
    }

    #[test]
    fn analog_axis_combines_with_keys_and_clamps() {
        let mut input = Input::with_default_map();
        // 摇杆推到一半：轴值连续（不是 ±1 的台阶）。
        input.state_mut().handle_pad_axis(0, GamepadAxis::LeftX, 0.62);
        let half = input.axis("MoveX");
        assert!(half > 0.4 && half < 0.8, "摇杆要给出连续值，实际 {half}");

        // 键盘同向再叠加：夹在 1.0，不会变成 1.62。
        press(&mut input, KeyCode::KeyD);
        assert_eq!(input.axis("MoveX"), 1.0);

        // 反向摇杆 + 正向键 = 抵消一部分，仍在范围内。
        input.state_mut().handle_pad_axis(0, GamepadAxis::LeftX, -0.5);
        let v = input.axis("MoveX");
        assert!((-1.0..=1.0).contains(&v), "轴值必须夹在 -1..=1，实际 {v}");
    }

    #[test]
    fn analog_deadzone_is_applied_and_rescaled() {
        let mut input = Input::with_default_map();
        // 死区 0.15 以内：不动。
        input.state_mut().handle_pad_axis(0, GamepadAxis::LeftY, 0.1);
        assert_eq!(input.axis("MoveY"), 0.0);
        // 推到底：必须正好 1.0（重标定，不是截断）。
        input.state_mut().handle_pad_axis(0, GamepadAxis::LeftY, 1.0);
        assert_eq!(input.axis("MoveY"), 1.0);
        // 向上为正（与 W 同向）。
        assert!(input.axis("MoveY") > 0.0);
    }

    #[test]
    fn multiple_pads_take_the_strongest_reading() {
        let mut input = Input::with_default_map();
        input.state_mut().handle_pad_axis(0, GamepadAxis::LeftX, 0.4);
        input.state_mut().handle_pad_axis(1, GamepadAxis::LeftX, -0.9);
        // 取绝对值最大的那台（-0.9），不是求和（-0.5）也不是第一台。
        assert!(input.pad_axis(GamepadAxis::LeftX) < -0.5);
        assert_eq!(input.pad_count(), 2);

        // 任意一台按下就成立。
        input.state_mut().handle_pad_button(1, GamepadButton::South, true);
        assert!(input.pad_button_down(GamepadButton::South));
    }

    #[test]
    fn disconnecting_a_pad_clears_its_state() {
        let mut input = Input::with_default_map();
        input.state_mut().pad_connected(7, "Pad");
        input.state_mut().handle_pad_button(7, GamepadButton::South, true);
        assert!(input.action_down("Jump"));
        input.state_mut().pad_disconnected(7);
        assert!(!input.action_down("Jump"), "拔掉之后不能还按着");
        assert_eq!(input.pad_count(), 0);
    }

    #[test]
    fn scene_reset_keeps_pads_connected_but_releases_buttons() {
        let mut input = Input::with_default_map();
        input.state_mut().pad_connected(0, "Pad");
        input.state_mut().handle_pad_button(0, GamepadButton::South, true);
        input.reset();
        assert_eq!(input.pad_count(), 1, "手柄是硬件事实，切场景不该丢");
        assert!(!input.pad_button_down(GamepadButton::South), "但按键要松开");
    }

    #[test]
    fn pad_defaults_are_added_once_and_never_override_author_bindings() {
        // 作者写过一条手柄绑定 ⇒ 一个默认值都不补。
        let mut authored = InputMap::new();
        authored.bind("Jump", Binding::Key(KeyCode::Space));
        authored.bind("Jump", Binding::PadButton(GamepadButton::North));
        let mapped = authored.clone().with_pad_defaults_if_absent();
        assert_eq!(mapped.actions["Jump"], authored.actions["Jump"]);

        // 完全没有手柄绑定 ⇒ 补上一整套，且不动已有的键位。
        let mut plain = InputMap::new();
        plain.bind("Jump", Binding::Key(KeyCode::Space));
        assert!(!plain.has_pad_bindings());
        let filled = plain.with_pad_defaults_if_absent();
        assert!(filled.has_pad_bindings());
        assert_eq!(
            filled.actions["Jump"].first(),
            Some(&Binding::Key(KeyCode::Space)),
            "补默认值不能把作者的键位挤掉（只能追加）"
        );
        assert!(filled.actions["Jump"].contains(&Binding::PadButton(GamepadButton::South)));
    }

    #[test]
    fn input_ron_without_analog_field_still_loads() {
        // 老项目文件里没有 `analog` 字段：必须能读，而且读出来是空表。
        let old = r#"(
            actions: { "Jump": [Key(Space)] },
            axes: { "MoveX": (positive: [Key(KeyD)], negative: [Key(KeyA)]) },
        )"#;
        let map: InputMap = ron::from_str(old).expect("老文件必须能读");
        assert!(map.axes["MoveX"].analog.is_empty());
    }

    #[test]
    fn empty_analog_is_not_written_to_disk() {
        // 写盘稳定性：没写 analog 的表保存后不该多出一行 `analog: []`。
        let mut map = InputMap::new();
        map.bind_axis(
            "MoveX",
            vec![Binding::Key(KeyCode::KeyD)],
            vec![Binding::Key(KeyCode::KeyA)],
        );
        let text = ron::ser::to_string_pretty(&map, ron::ser::PrettyConfig::new()).unwrap();
        assert!(!text.contains("analog"), "空模拟表不落盘：{text}");

        let full = InputMap::default_game();
        let text = ron::ser::to_string_pretty(&full, ron::ser::PrettyConfig::new()).unwrap();
        let back: InputMap = ron::from_str(&text).expect("带模拟轴的映射必须能读回");
        assert!(!back.axes["MoveX"].analog.is_empty());
    }

    #[test]
    fn ron_roundtrip() {
        let map = InputMap::default_game();
        let ron = ron::ser::to_string_pretty(&map, ron::ser::PrettyConfig::new()).unwrap();
        let back: InputMap = ron::from_str(&ron).unwrap();
        assert_eq!(back.actions.len(), map.actions.len());
        assert_eq!(back.axes.len(), map.axes.len());
    }
}
