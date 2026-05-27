//! 中央编辑器状态——拥有场景、选择、相机和工具设置。
//!
//! 所有编辑器面板从该状态读取并通过它写入，保持引擎数据模型和编辑器 UI 同步。

use std::collections::HashMap;

use xgame_math::glam::Vec3;
use xgame_math::Transform;
use xgame_physics::{
    Collider, PhysicsConfig, PhysicsWorld, RigidBody,
};
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

/// Top-level editor state shared across all panels.
pub struct EditorState {
    // -- Scene --
    /// The currently loaded scene (entities + metadata).
    pub scene: Scene,
    /// Index of the selected entity in `scene.entities`; `None` = nothing selected.
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
    /// Viewport size in logical (egui) pixels.
    pub viewport_logical_size: egui::Vec2,
    /// Viewport size in physical pixels (= logical * pixels_per_point).
    pub viewport_pixel_size: (u32, u32),

    // -- Fly mode state --
    /// `true` when the user holds right-click over the viewport.
    /// Stays `true` until right-click is released, even if the mouse leaves the viewport.
    /// Guards WASD/QE fly movement in scene_view.
    pub fly_state: bool,

    /// Countdown timer (seconds) for the zoom speed overlay.
    /// When > 0, scene_view draws "Speed: X.X" on the viewport.
    pub zoom_indicator: f32,

    // -- Scene renderer (initialised by main.rs after state creation) --
    /// Optional 3D scene renderer — `None` if the wgpu backend is unavailable.
    pub scene_renderer: Option<crate::scene_renderer::SceneRenderer>,

    // -- Physics (play mode) --
    /// Physics world — `Some` only during Play mode.
    pub physics_world: Option<PhysicsWorld>,
    /// Maps entity index → physics body user ID.
    pub entity_to_body: HashMap<usize, u64>,
    /// Saved entity transforms before entering Play, for restore on Stop.
    saved_states: Vec<Transform>,
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
            tool_mode: ToolMode::Translate,
            play_mode: PlayMode::Edit,
            show_grid: true,
            show_physics_debug: false,
            console_messages: console,
            next_entity_id: 1,

            viewport_logical_size: egui::Vec2::ZERO,
            viewport_pixel_size: (0, 0),

            fly_state: false,
            zoom_indicator: 0.0,

            // Scene renderer is injected by main.rs after state creation
            scene_renderer: None,

            // Physics (initially None — created on Play)
            physics_world: None,
            entity_to_body: HashMap::new(),
            saved_states: Vec::new(),
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

    // -----------------------------------------------------------------------
    // Physics (play mode)
    // -----------------------------------------------------------------------

    /// Enter Play mode: save entity states, create physics world, add bodies.
    pub fn enter_play_mode(&mut self) {
        // Save every entity's current transform for later restore
        self.saved_states = self.scene.entities.iter().map(|e| e.transform).collect();

        // Create a fresh physics world
        let mut physics = PhysicsWorld::new(PhysicsConfig::default());

        // For each entity with a model, create a dynamic rigid body + cuboid collider
        self.entity_to_body.clear();
        for (i, entity) in self.scene.entities.iter().enumerate() {
            if entity.model.is_empty() {
                continue; // camera, light, empty objects skip physics
            }
            let body = RigidBody::dynamic_at(entity.transform);
            let body_id = physics.add_body(body);

            let half_extents = (entity.transform.scale * 0.5).max(Vec3::splat(0.01));
            let collider = Collider::cuboid(half_extents);
            physics.add_collider(body_id, collider);

            self.entity_to_body.insert(i, body_id);
        }

        self.physics_world = Some(physics);
        self.log_info("Entered Play mode — physics simulation started");
    }

    /// Exit Play mode: restore saved transforms, clear physics world.
    pub fn exit_play_mode(&mut self) {
        // Restore every entity's pre-play transform
        for (i, transform) in self.saved_states.iter().enumerate() {
            if let Some(entity) = self.scene.entities.get_mut(i) {
                entity.transform = *transform;
            }
        }

        self.physics_world = None;
        self.entity_to_body.clear();
        self.saved_states.clear();
        self.log_info("Returned to Edit mode — states restored");
    }

    /// Advance physics by `dt` seconds and sync transforms back to entities.
    ///
    /// Only call when `play_mode == PlayMode::Playing`.
    pub fn physics_step(&mut self, dt: f32) {
        let physics = match self.physics_world.as_mut() {
            Some(p) => p,
            None => return,
        };
        physics.step(dt);

        // Sync position+rotation from physics bodies back to entities
        // (scale is preserved from the current entity — physics has no scale concept)
        let mapping: Vec<(usize, u64)> =
            self.entity_to_body.iter().map(|(&i, &b)| (i, b)).collect();
        for (entity_idx, body_id) in mapping {
            if let Some(phys_transform) = physics.body_transform(body_id) {
                if let Some(entity) = self.scene.entities.get_mut(entity_idx) {
                    entity.transform.translation = phys_transform.translation;
                    entity.transform.rotation = phys_transform.rotation;
                    // scale intentionally NOT overwritten
                }
            }
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
