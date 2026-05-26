//! 中央编辑器状态——拥有场景、选择、相机和工具设置。
//!
//! 所有编辑器面板从该状态读取并通过它写入，保持引擎数据模型和编辑器 UI 同步。

use std::collections::HashSet;
use xgame_math::glam::Vec3;
use xgame_math::Transform;
use xgame_render::scene::{Entity, Scene};
use strum::Display;

/// Which transform tool is active in the scene view.
#[derive(Debug, Clone, Copy, Display, PartialEq, Eq)]
pub enum ToolMode {
    Translate,
    Rotate,
    Scale,
    Rect,
}

/// Editor playback state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayMode {
    Edit,
    Playing,
    Paused,
}

/// Log severity for the console panel.
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


/// A single console message.
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

/// Free‑fly camera — yaw/pitch controls orientation, `pan_*` moves position.
#[derive(Debug, Clone)]
pub struct EditorCamera {
    pub yaw: f32,
    pub pitch: f32,
    /// World‑space camera position.
    pub position: Vec3,
    pub sensitivity: f32,
    /// Translation speed (world-units / s).
    pub pan_speed: f32,
}

impl Default for EditorCamera {
    fn default() -> Self {
        Self {
            yaw: -2.356,                // ≈ −135° — face the origin from (+, +, +)
            pitch: -0.524,              // ≈ −30°  — slight downward look
            position: Vec3::new(5.0, 5.0, 5.0),
            sensitivity: 0.005,
            pan_speed: 5.0,
        }
    }
}

impl EditorCamera {
    /// Unit forward vector derived from yaw/pitch.
    pub fn forward(&self) -> Vec3 {
        Vec3::new(
            self.pitch.cos() * self.yaw.cos(),
            self.pitch.sin(),
            self.pitch.cos() * self.yaw.sin(),
        )
        .normalize()
    }

    /// Eye position (= position in free‑fly mode).
    pub fn eye(&self) -> Vec3 {
        self.position
    }

    pub fn rotate(&mut self, dx: f32, dy: f32) {
        self.yaw += dx * self.sensitivity;
        self.pitch -= dy * self.sensitivity;   // egui dy is +down, so negate
        self.pitch = self.pitch.clamp(
            -std::f32::consts::FRAC_PI_2 + 0.01,
            std::f32::consts::FRAC_PI_2 - 0.01,
        );
    }

    /// Adjust movement speed with scroll wheel.
    pub fn zoom(&mut self, delta: f32) {
        self.pan_speed = (self.pan_speed + delta * 0.5).clamp(0.5, 50.0);
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

/// Top-level editor state shared across all panels.
pub struct EditorState {
    // -- Scene --
    /// The currently loaded scene (entities + metadata).
    pub scene: Scene,
    /// Index of the selected entity in `scene.entities`; `None` = nothing selected.
    pub selected_entity: Option<usize>,

    // -- Editor camera --
    pub editor_camera: EditorCamera,

    // -- Camera input state (self-tracked to bypass OS key-repeat delay) --
    /// Set of keys currently held down, updated from `KeyPressed`/`KeyReleased` events.
    pub pressed_keys: HashSet<egui::Key>,

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
    /// Viewport size in logical (egui) pixels.
    pub viewport_logical_size: egui::Vec2,
    /// Viewport size in physical pixels (= logical * pixels_per_point).
    pub viewport_pixel_size: (u32, u32),

    // -- Scene renderer (initialised by main.rs after state creation) --
    /// Optional 3D scene renderer — `None` if the wgpu backend is unavailable.
    pub scene_renderer: Option<crate::scene_renderer::SceneRenderer>,
}

impl EditorState {
    /// Create a fresh editor state with a default scene and a welcome console message.
    pub fn new() -> Self {
        let mut console = Vec::new();
        console.push(LogMessage::new(LogLevel::Success, "XGameEngine Editor started"));
        console.push(LogMessage::new(LogLevel::Info, "Loaded default scene"));

        // 尝试加载 Scenes/scene.ron；失败时回退到默认场景。
        let scene = load_scene_from_file("Scenes/scene.ron")
            .unwrap_or_else(|e| {
                log::warn!("无法加载 Scenes/scene.ron: {:#}", e);
                default_editor_scene()
            });

        Self {
            scene,
            selected_entity: None,
            editor_camera: EditorCamera::default(),
            pressed_keys: HashSet::new(),
            tool_mode: ToolMode::Translate,
            play_mode: PlayMode::Edit,
            show_grid: true,
            show_physics_debug: false,
            console_messages: console,
            next_entity_id: 1,

            viewport_logical_size: egui::Vec2::ZERO,
            viewport_pixel_size: (0, 0),

            // Scene renderer is injected by main.rs after state creation
            scene_renderer: None,
        }
    }

    // -- Selection helpers --

    /// Return a mutable reference to the selected entity, if any.
    pub fn selected_entity_mut(&mut self) -> Option<&mut Entity> {
        self.selected_entity
            .and_then(|i| self.scene.entities.get_mut(i))
    }

    /// Return a reference to the selected entity, if any.
    pub fn selected_entity_ref(&self) -> Option<&Entity> {
        self.selected_entity
            .and_then(|i| self.scene.entities.get(i))
    }

    // -- Console helpers --

    pub fn log_info(&mut self, text: impl Into<String>) {
        self.console_messages
            .push(LogMessage::new(LogLevel::Info, text));
    }

    pub fn log_warning(&mut self, text: impl Into<String>) {
        self.console_messages
            .push(LogMessage::new(LogLevel::Warning, text));
    }

    pub fn log_error(&mut self, text: impl Into<String>) {
        self.console_messages
            .push(LogMessage::new(LogLevel::Error, text));
    }

    pub fn log_success(&mut self, text: impl Into<String>) {
        self.console_messages
            .push(LogMessage::new(LogLevel::Success, text));
    }

    // -- Entity manipulation --

    /// Add a new empty entity to the scene and return its index.
    pub fn create_entity(&mut self) -> usize {
        let id = self.next_entity_id;
        self.next_entity_id += 1;
        let idx = self.scene.entities.len();
        self.scene.entities.push(Entity {
            name: format!("GameObject ({})", id),
            transform: Transform::IDENTITY,
            model: String::new(),
        });
        self.selected_entity = Some(idx);
        self.log_info(format!("Created new GameObject ({})", id));
        idx
    }

    /// Duplicate the currently selected entity.
    pub fn duplicate_entity(&mut self) {
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

    /// Delete the currently selected entity.
    pub fn delete_selected(&mut self) {
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
}

impl Default for EditorState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Scene loading from RON file
// ---------------------------------------------------------------------------

/// Load a `Scene` from a `.ron` file on disk.
///
/// Returns an error string if the file cannot be read or parsed.
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
            Entity {
                name: "Main Camera".into(),
                transform: Transform::from_translation(Vec3::new(0.0, 1.0, -5.0)),
                model: String::new(),
            },
            Entity {
                name: "Directional Light".into(),
                transform: Transform::IDENTITY,
                model: String::new(),
            },
        ],
    }
}
