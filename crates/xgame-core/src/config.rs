/// 引擎顶层配置。
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub window: WindowConfig,
    /// 目标帧率。0 = 不限制。
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

/// 窗口创建设置。
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
