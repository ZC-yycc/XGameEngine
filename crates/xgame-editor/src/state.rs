//! 中央编辑器状态——拥有场景、选择、相机和工具设置。
//!
//! 所有编辑器面板从该状态读取并通过它写入，保持引擎数据模型和编辑器 UI 同步。

use xgame_math::glam::Vec3;
use xgame_core::{Entity, Scene, Transform};
use xgame_physics::DebugLine;
use strum::Display;
use crate::editor_input::EditorInputManager;


/// 场景视图中当前激活的变换工具。
#[derive(Debug, Clone, Copy, Display, PartialEq, Eq)]
pub enum ToolMode {
    Translate,
    Rotate,
    Scale,
    Rect,
}

/// 编辑器播放状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayMode {
    Edit,
    Playing,
    Paused,
}

/// Gizmo 交互操作柄标识（与 ToolMode 组合使用）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GizmoPart {
    AxisX,
    AxisY,
    AxisZ,
    PlaneXY,
    PlaneXZ,
    PlaneYZ,
    Uniform,
    RingX,
    RingY,
    RingZ,
}

/// 活动拖拽状态。
#[derive(Debug, Clone)]
pub struct GizmoDrag {
    /// 被拖拽的操作柄。
    pub part: GizmoPart,
    /// 拖拽起始时 entity transform 的对应值（translation / scale 的 Vec3，或 rotation Euler）。
    pub start_value: Vec3,
    /// 拖拽平面上首次命中点的世界坐标。
    pub start_hit: Vec3,
    pub start_screen_pos: egui::Pos2,
}

/// 控制台面板的日志级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warning,
    Error,
    Success,
}

impl LogLevel {
    pub fn log_level_color(&self) -> egui::Color32 {
        match self {
            LogLevel::Info => egui::Color32::WHITE,
            LogLevel::Warning => egui::Color32::YELLOW,
            LogLevel::Error => egui::Color32::RED,
            LogLevel::Success => egui::Color32::GREEN,
        }
    }

    pub fn log_level_icon(&self) -> &'static str {
        match self {
            LogLevel::Info => "ℹ",
            LogLevel::Warning => "⚠",
            LogLevel::Error => "✖",
            LogLevel::Success => "✔",
        }
    }
}


/// 单条控制台消息。
#[derive(Debug, Clone)]
pub struct LogMessage {
    pub level: LogLevel,
    pub text: String,
    pub count: u32,
}

impl LogMessage {
    pub fn new(level: LogLevel, text: impl Into<String>) -> Self {
        Self {
            level,
            text: text.into(),
            count: 1,
        }
    }
}

/// 自由飞行相机——yaw/pitch 控制朝向，`pan_*` 移动位置。
#[derive(Debug, Clone)]
pub struct EditorCamera {
    pub yaw: f32,
    pub pitch: f32,
    /// 世界空间中的相机位置。
    pub position: Vec3,
    pub sensitivity: f32,
    /// 平移速度（世界单位/秒）。
    pub pan_speed: f32,
}

impl Default for EditorCamera {
    fn default() -> Self {
        Self {
            yaw: -2.356,                // ≈ −135° — 从 (+, +, +) 方向看向原点
            pitch: -0.524,              // ≈ −30°  — 略微向下
            position: Vec3::new(5.0, 5.0, 5.0),
            sensitivity: 0.0025,
            pan_speed: 5.0,
        }
    }
}

impl EditorCamera {
    
    /// 由 yaw/pitch 计算的前方向单位向量。
    pub fn forward(&self) -> Vec3 {
        Vec3::new(
            self.pitch.cos() * self.yaw.cos(),
            self.pitch.sin(),
            self.pitch.cos() * self.yaw.sin(),
        )
        .normalize()
    }

    /// 观察点位置（自由飞行模式下等于 position）。
    pub fn eye(&self) -> Vec3 {
        self.position
    }

    pub fn rotate(&mut self, dx: f32, dy: f32) {
        self.yaw += dx * self.sensitivity;
        self.pitch -= dy * self.sensitivity;   // egui 的 dy 向下为正，所以取反
        self.pitch = self.pitch.clamp(
            -std::f32::consts::FRAC_PI_2 + 0.01,
            std::f32::consts::FRAC_PI_2 - 0.01,
        );
    }

    /// 用滚轮调整移动速度。
    pub fn zoom(&mut self, delta: f32) {
        self.pan_speed = (self.pan_speed + delta * 0.05).clamp(0.5, 50.0);
    }

    /// 沿相机右方向平移（A / D）— 移动 position。
    pub fn pan_right(&mut self, amount: f32, dt: f32) {
        let fwd = self.forward();
        let right = fwd.cross(Vec3::Y).normalize();
        self.position += right * amount * self.pan_speed * dt;
    }

    /// 沿相机前方向的水平投影平移（W / S）— 移动 position。
    pub fn pan_forward(&mut self, amount: f32, dt: f32) {
        let fwd = self.forward();
        let fwd_h = Vec3::new(fwd.x, 0.0, fwd.z).normalize_or_zero();
        self.position += fwd_h * amount * self.pan_speed * dt;
    }

    /// 沿世界空间上/下方向升降（Q / E）。
    pub fn move_up(&mut self, amount: f32, dt: f32) {
        self.position.y += amount * self.pan_speed * dt;
    }
}

// ---------------------------------------------------------------------------
// Editor state
// ---------------------------------------------------------------------------

/// 所有面板共享的顶层编辑器状态。
pub struct EditorState {
    // -- Scene --
    /// 当前加载的场景（实体 + 元数据）。
    pub scene: Scene,
    /// 当前选中实体在 `scene.entities` 中的索引；`None` = 未选中。
    pub selected_entity: Option<usize>,

    // -- Editor camera --
    pub editor_camera: EditorCamera,

    // -- Tool state --
    pub tool_mode: ToolMode,
    pub play_mode: PlayMode,

    // -- Toggles --
    pub show_grid: bool,
    pub show_physics_debug: bool,

    // -- Console --
    pub console_messages: Vec<LogMessage>,

    // -- New entity tracking --
    pub next_entity_id: u32,

    // -- Scene view (viewport dimensions, set by scene_view.rs each frame) --
    /// 视口大小（egui 逻辑像素）。
    pub viewport_logical_size: egui::Vec2,
    /// 视口大小（物理像素 = 逻辑像素 x pixels_per_point）。
    pub viewport_pixel_size: (u32, u32),

    // -- Fly mode state --
    /// 用户在视口上按住右键时为 `true`。
    /// 即使鼠标离开视口，在右键释放前保持 `true`。
    /// 控制 scene_view 中 WASD/QE 飞行移动的开关。
    pub fly_state: bool,

    /// 缩放速度叠加层的倒计时（秒）。
    /// 大于 0 时，scene_view 在视口上显示 "Speed: X.X"。
    pub zoom_indicator: f32,

    // -- Scene renderer (initialised by main.rs after state creation) --
    /// 可选的 3D 场景渲染器——wgpu 后端不可用时为 `None`。
    pub scene_renderer: Option<crate::scene_renderer::SceneRenderer>,

    // -- Game runtime (play mode) --
    /// 游戏运行时——仅在 Play 模式下为 `Some`。
    /// 拥有物理世界并管理实体-刚体映射。
    pub game_runtime: Option<crate::game_runtime::GameRuntime>,

    // -- Gizmo state --
    /// 当前鼠标悬停的 gizmo 操作柄（用于高亮绘制）。
    pub gizmo_hovered: Option<GizmoPart>,
    /// 当前正在拖拽的操作柄状态（`Some` 时处于拖拽中）。
    pub gizmo_active: Option<GizmoDrag>,
    /// 拖拽捕捉步长（0 表示不捕捉）。
    #[allow(dead_code)]
    pub gizmo_snap: f32,
    
    /// 输入管理器：跟踪键盘/鼠标状态以支持全局快捷键和复杂输入。
    pub input_manager: EditorInputManager,
}

impl EditorState {
    pub fn new() -> Self {
        let scene = match load_scene_from_file("Scenes/scene.ron") {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[EditorState] {e}");
                default_editor_scene()
            }
        };
        Self {
            scene,
            selected_entity: None,

            editor_camera: EditorCamera::default(),
            tool_mode: ToolMode::Translate,
            play_mode: PlayMode::Edit,
            show_grid: true,
            show_physics_debug: false,

            console_messages: Vec::new(),
            next_entity_id: 1,

            viewport_logical_size: egui::Vec2::ZERO,
            viewport_pixel_size: (0, 0),

            fly_state: false,
            zoom_indicator: 0.0,

            scene_renderer: None,
            game_runtime: None,

            gizmo_hovered: None,
            gizmo_active: None,
            gizmo_snap: 0.0,

            input_manager: EditorInputManager::new(),
        }
    }

    /// 是否处于编辑模式（非 Play 且非 Pause）。
    pub fn is_editing(&self) -> bool {
        self.play_mode == PlayMode::Edit
    }

    pub fn entity(&self) -> Option<&Entity> {
        self.selected_entity.map(|i| &self.scene.entities[i])
    }

    pub fn entity_mut(&mut self) -> Option<&mut Entity> {
        let i = self.selected_entity?;
        self.scene.entities.get_mut(i)
    }

    // -- Console helpers --

    pub fn log_info(&mut self, text: impl Into<String>) {
        self.log(LogLevel::Info, text);
    }

    pub fn log_warning(&mut self, text: impl Into<String>) {
        self.log(LogLevel::Warning, text);
    }

    pub fn log_error(&mut self, text: impl Into<String>) {
        self.log(LogLevel::Error, text);
    }

    pub fn log_success(&mut self, text: impl Into<String>) {
        self.log(LogLevel::Success, text);
    }

    pub fn log(&mut self, level: LogLevel, text: impl Into<String>) {
        let text: String = text.into();
        // Deduplicate consecutive identical messages
        if let Some(last) = self.console_messages.last_mut() {
            if last.level == level && last.text == text {
                last.count += 1;
                return;
            }
        }
        self.console_messages
            .push(LogMessage::new(level, text));
    }

    // -- 实体操作 --

    /// 向场景添加一个新的空实体并返回其索引。
    ///
    /// 在 Play 模式下不可用。
    pub fn create_entity(&mut self) -> usize {
        if !self.is_editing() {
            self.log_warning("Cannot create entities while playing");
            return usize::MAX;
        }
        let id = self.next_entity_id;
        self.next_entity_id += 1;
        let idx = self.scene.entities.len();
        self.scene.entities.push(
            Entity::new(format!("GameObject ({})", id)),
        );
        self.selected_entity = Some(idx);
        self.log_info(format!("Created new GameObject ({})", id));
        idx
    }

    /// 复制当前选中的实体。
    ///
    /// 在 Play 模式下不可用。
    pub fn duplicate_entity(&mut self) {
        if !self.is_editing() {
            self.log_warning("Cannot duplicate entities while playing");
            return;
        }
        let idx = match self.selected_entity {
            Some(i) => i,
            None => return,
        };
        let mut clone = self.scene.entities[idx].clone();
        clone.name = format!("{} (Copy)", clone.name);
        self.scene.entities.push(clone);
        self.selected_entity = Some(self.scene.entities.len() - 1);
        self.log_info("Duplicated entity");
    }

    /// 删除当前选中的实体。
    ///
    /// 在 Play 模式下不可用。
    pub fn delete_selected(&mut self) {
        if !self.is_editing() {
            self.log_warning("Cannot delete entities while playing");
            return;
        }
        if let Some(idx) = self.selected_entity {
            let removed = self.scene.entities.remove(idx);
            self.log_info(format!("Deleted '{}'", removed.name));
            self.selected_entity = if self.scene.entities.is_empty() {
                None
            } else {
                Some(idx.min(self.scene.entities.len() - 1))
            };
        }
    }

    // -----------------------------------------------------------------------
    // Game runtime (play mode)
    // -----------------------------------------------------------------------

    /// 进入播放模式：启动游戏运行时并记录日志。
    pub fn enter_play_mode(&mut self) {
        let runtime = crate::game_runtime::GameRuntime::start(&self.scene.entities);
        self.game_runtime = Some(runtime);
        self.log_info("Entered Play mode — physics simulation started");
    }

    /// 退出播放模式：停止游戏运行时，恢复变换，记录日志。
    pub fn exit_play_mode(&mut self) {
        if let Some(runtime) = self.game_runtime.take() {
            runtime.stop(&mut self.scene.entities);
        }
        self.log_info("Returned to Edit mode — states restored");
    }

    /// 将物理模拟推进 `dt` 秒（委托给 GameRuntime）。
    ///
    /// 仅在 `play_mode == PlayMode::Playing` 时调用。
    pub fn physics_step(&mut self, dt: f32) {
        if let Some(runtime) = self.game_runtime.as_mut() {
            runtime.fixed_update(dt, &mut self.scene.entities);
        }
    }

    /// 获取物理调试线条（用于 scene_view 叠加绘制）。
    ///
    /// 仅在 `show_physics_debug` 为 true 时调用。
    pub fn debug_lines(&self) -> Vec<DebugLine> {
        if self.show_physics_debug {
            if let Some(runtime) = self.game_runtime.as_ref() {
                return runtime.debug_lines();
            }
        }
        Vec::new()
    }

    /// 获取物理统计（刚体数, 碰撞体数）。
    pub fn physics_stats(&self) -> (usize, usize) {
        self.game_runtime.as_ref().map_or((0, 0), |rt| rt.stats())
    }
}

impl Default for EditorState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// 从 RON 文件加载场景
// ---------------------------------------------------------------------------

/// 从磁盘上的 `.ron` 文件加载 `Scene`。
///
/// 如果文件无法读取或解析，返回错误字符串。
fn load_scene_from_file(path: &str) -> Result<Scene, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("无法读取 '{}': {}", path, e))?;
    ron::from_str(&content)
        .map_err(|e| format!("无法解析 '{}': {:#}", path, e))
}

/// 创建编辑器默认场景（空场景 + 相机 + 方向光）。
pub fn default_editor_scene() -> Scene {
    Scene {
        name: "Untitled Scene".into(),
        entities: vec![
            Entity::new("Main Camera")
                .with_transform(Transform::from_translation(Vec3::new(0.0, 1.0, -5.0))),
            Entity::new("Directional Light"),
        ],
    }
}
