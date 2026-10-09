//! 引擎配置类型。
//!
//! 提供窗口设置、引擎顶层选项，以及资源路径配置。
//! 所有配置类型均支持 `Serialize` / `Deserialize`，可通过 RON 文件持久化。

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// 窗口配置
// ---------------------------------------------------------------------------

/// 窗口创建设置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowConfig {
    /// 窗口标题。
    pub title: String,
    pub width: u32,
    pub height: u32,
    /// 是否允许用户调整窗口大小。
    pub resizable: bool,
    /// 是否启用垂直同步。
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

// ---------------------------------------------------------------------------
// 引擎顶层配置
// ---------------------------------------------------------------------------

/// 引擎顶层配置。
///
/// `Default` 直接派生：每个字段的默认值就是它自己 `Default::default()` 的结果，
/// 手写一份只是把同样的东西再抄一遍，抄漏一个字段还会静默变成"和别处不一样"。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EngineConfig {
    pub window: WindowConfig,
    /// 目标帧率。0 = 不限制。
    pub target_fps: u32,
    #[serde(default)]
    pub content: ContentConfig,
}

// ---------------------------------------------------------------------------
// 内容/资源路径配置
// ---------------------------------------------------------------------------

/// 内容资源路径配置。
///
/// 控制引擎搜索资产文件的目录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentConfig {
    /// 场景文件目录。
    pub scenes_dir: String,
    /// 模型/网格资产目录。
    pub models_dir: String,
    /// 纹理资产目录。
    pub textures_dir: String,
    /// 音频资产目录。
    pub audio_dir: String,
    /// 着色器资产目录。
    #[serde(default = "default_shaders_dir")]
    pub shaders_dir: String,
    /// 预制体资产目录。
    ///
    /// 带 `serde(default)`：这个字段是后加的，老项目的 `project.ron` 里没有它，
    /// 反序列化时补默认值即可，不必强制升级清单文件。
    #[serde(default = "default_prefabs_dir")]
    pub prefabs_dir: String,
    /// 脚本目录（`.rhai` 文件）。
    ///
    /// 同样带 serde 默认值：脚本是后加的能力，老清单里没有这一项。
    /// 运行时也**只**用默认值兜底，不会因为目录不存在就报错 ——
    /// 一个还没写脚本的项目完全合法。
    #[serde(default = "default_scripts_dir")]
    pub scripts_dir: String,
    /// 附加搜索路径列表。
    pub search_paths: Vec<String>,
}

fn default_shaders_dir() -> String {
    "Shaders".into()
}

fn default_prefabs_dir() -> String {
    "Prefabs".into()
}

fn default_scripts_dir() -> String {
    "Scripts".into()
}

impl Default for ContentConfig {
    fn default() -> Self {
        Self {
            scenes_dir: "Scenes".into(),
            models_dir: "Resources".into(),
            textures_dir: "Resources".into(),
            audio_dir: "Resources".into(),
            shaders_dir: default_shaders_dir(),
            prefabs_dir: default_prefabs_dir(),
            scripts_dir: default_scripts_dir(),
            search_paths: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_window_config() {
        let cfg = WindowConfig::default();
        assert_eq!(cfg.title, "XGameEngine");
        assert!(cfg.resizable);
    }

    #[test]
    fn serde_roundtrip() {
        let cfg = EngineConfig::default();
        let ron_str = ron::to_string(&cfg).expect("serialize");
        let restored: EngineConfig = ron::from_str(&ron_str).expect("deserialize");
        assert_eq!(restored.target_fps, 0);
        assert_eq!(restored.window.width, 1280);
    }
}
