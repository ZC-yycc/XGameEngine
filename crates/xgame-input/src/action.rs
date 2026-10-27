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
use crate::state::InputState;

/// 一条按键绑定。
///
/// 用 `BTreeMap` 而非 `HashMap` 承载动作表，是为了**序列化结果稳定**：
/// 同一个 `InputMap` 存两次得到字节相同的文件，diff 才干净、才适合进版本库。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
        }
    }

    /// 本帧是否刚松开。
    fn is_released(&self, state: &InputState) -> bool {
        match self {
            Binding::Key(k) => state.key_released(*k),
            Binding::Mouse(m) => state.mouse_released(*m),
            Binding::Combo { key, .. } => state.key_released(*key),
        }
    }
}

/// 一条双向轴（例如 `MoveX = D - A`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxisBinding {
    /// 正方向按键（+1）。
    #[serde(default)]
    pub positive: Vec<Binding>,
    /// 负方向按键（-1）。
    #[serde(default)]
    pub negative: Vec<Binding>,
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
        self.axes
            .insert(axis.into(), AxisBinding { positive, negative });
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

        map
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
    pub fn save(&self, path: &Path) -> Result<(), InputMapError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(InputMapError::Io)?;
        }
        let ron = ron::ser::PrettyConfig::new()
            .depth_limit(4)
            .enumerate_arrays(true);
        let text = ron::ser::to_string_pretty(self, ron)
            .map_err(|e| InputMapError::Parse(e.to_string()))?;
        use std::io::Write;
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_STAGING: AtomicU64 = AtomicU64::new(0);
        // 每次独占一个同目录暂存名，旧崩溃残留不能挡住新保存。
        let (tmp, mut file) = loop {
            let id = NEXT_STAGING.fetch_add(1, Ordering::Relaxed);
            let tmp = path.with_extension(match path.extension().and_then(|e| e.to_str()) {
                Some(ext) => format!("{ext}.tmp.{}.{id}", std::process::id()),
                None => format!("tmp.{}.{id}", std::process::id()),
            });
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp) {
                Ok(file) => break (tmp, file),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(InputMapError::Io(e)),
            }
        };
        let result = file.write_all(text.as_bytes()).and_then(|_| file.sync_all());
        drop(file);
        let result = result.and_then(|_| std::fs::rename(&tmp, path));
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result.map_err(InputMapError::Io)
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
        (pos - neg) as f32
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
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let mut old = InputMap::new();
        old.bind("Old", Binding::Key(KeyCode::KeyA));
        old.save(&path).unwrap();
        let before = std::fs::read(&path).unwrap();
        // 旧版本的固定暂存残留也不得阻断保存，更不能被这次失败清掉。
        let legacy_tmp = path.with_extension("ron.tmp");
        std::fs::write(&legacy_tmp, b"incomplete previous save").unwrap();
        let lock = std::fs::OpenOptions::new().read(true).share_mode(3).open(&path).unwrap();
        let mut new = InputMap::new();
        new.bind("New", Binding::Key(KeyCode::KeyB));
        assert!(new.save(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(std::fs::read(&legacy_tmp).unwrap(), b"incomplete previous save");
        drop(lock);
        new.save(&path).unwrap();
        assert!(InputMap::load(&path).unwrap().unwrap().actions.contains_key("New"));
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(legacy_tmp).unwrap();
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
