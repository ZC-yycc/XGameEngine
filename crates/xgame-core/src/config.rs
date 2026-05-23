/// Top-level engine configuration.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub window: WindowConfig,
    /// Target frames per second. 0 = uncapped.
    pub target_fps: u32,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            window: WindowConfig::default(),
            target_fps: 0,
        }
    }
}

/// Window creation settings.
#[derive(Debug, Clone)]
pub struct WindowConfig {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub resizable: bool,
    pub vsync: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "XGameEngine".into(),
            width: 1280,
            height: 720,
            resizable: true,
            vsync: true,
        }
    }
}
