//! Central editor state — owns the scene, selection, camera, and tool settings.
//!
//! All editor panels read from this state and write back through it,
//! keeping the engine data model and the editor UI in sync.

use eframe::glow::STENCIL_FAIL;
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

/// Simplified editor orbit camera — used by the scene-view panel.
#[derive(Debug, Clone)]
pub struct EditorCamera {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub target: Vec3,
    pub sensitivity: f32,
    pub zoom_speed: f32,
}

impl Default for EditorCamera {
    fn default() -> Self {
        Self {
            yaw: std::f32::consts::FRAC_PI_4,
            pitch: std::f32::consts::FRAC_PI_6,
            distance: 8.0,
            target: Vec3::ZERO,
            sensitivity: 0.005,
            zoom_speed: 1.0,
        }
    }
}

impl EditorCamera {
    pub fn eye(&self) -> Vec3 {
        let x = self.distance * self.pitch.cos() * self.yaw.cos();
        let y = self.distance * self.pitch.sin();
        let z = self.distance * self.pitch.cos() * self.yaw.sin();
        self.target + Vec3::new(x, y, z)
    }

    pub fn rotate(&mut self, dx: f32, dy: f32) {
        self.yaw += dx * self.sensitivity;
        self.pitch += dy * self.sensitivity;
        self.pitch = self.pitch.clamp(
            -std::f32::consts::FRAC_PI_2 + 0.01,
            std::f32::consts::FRAC_PI_2 - 0.01,
        );
    }

    pub fn zoom(&mut self, delta: f32) {
        self.distance = (self.distance - delta * self.zoom_speed).max(0.3);
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
}

impl EditorState {
    /// Create a fresh editor state with a default scene and a welcome console message.
    pub fn new() -> Self {
        let mut console = Vec::new();
        console.push(LogMessage::new(LogLevel::Success, "XGameEngine Editor started"));
        console.push(LogMessage::new(LogLevel::Info, "Loaded default scene"));

        Self {
            scene: Scene {
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
            },
            selected_entity: None,
            editor_camera: EditorCamera::default(),
            tool_mode: ToolMode::Translate,
            play_mode: PlayMode::Edit,
            show_grid: true,
            show_physics_debug: false,
            console_messages: console,
            next_entity_id: 1,
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
