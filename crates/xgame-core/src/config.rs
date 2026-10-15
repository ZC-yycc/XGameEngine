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
    /// 脚本目录（`.lua` 文件）。
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
// 运行时参数
// ---------------------------------------------------------------------------

/// 玩法层可调参数（重力 / 固定步长），持久化在 `project.ron` 的 `runtime` 节。
///
/// 这两个值此前硬编码在 `xgame-runtime::WorldConfig` 的默认值里 —— 想调重力
/// 只能改引擎源码重编译。现在进清单、由编辑器「⚙ Project Settings」面板编辑；
/// `Project` 上的 `#[serde(default)]` 保证旧清单照常加载。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RuntimeConfig {
    /// 世界重力（m/s²，Y 向上、向下为负）。
    pub gravity: [f32; 3],
    /// 物理 / 固定步脚本的步长（秒）。
    pub fixed_dt: f32,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            gravity: [0.0, -9.81, 0.0],
            fixed_dt: 1.0 / 60.0,
        }
    }
}

impl RuntimeConfig {
    /// 容错读取：清单是**手改的文本**，非法值不能直接交给物理引擎。
    ///
    /// - 重力含 NaN / 无穷 / 超大值（>1e6，一帧能把物体弹出银河系）→ 退回默认；
    /// - `fixed_dt <= 0` 会让固定步长累加器永远凑不满一步（时间静止），非有限
    ///   同理 → 退回默认；
    /// - 合法但离谱的步长夹进 [1/240s, 1/10s]：比 240Hz 更快没有意义，
    ///   比 10Hz 更慢肉眼可见地卡。
    pub fn sanitized(self) -> Self {
        let sane = |v: [f32; 3]| v.iter().all(|x| x.is_finite() && x.abs() <= 1e6);
        let gravity = if sane(self.gravity) {
            self.gravity
        } else {
            Self::default().gravity
        };
        let fixed_dt = if self.fixed_dt.is_finite() && self.fixed_dt > 0.0 {
            self.fixed_dt.clamp(1.0 / 240.0, 1.0 / 10.0)
        } else {
            Self::default().fixed_dt
        };
        Self { gravity, fixed_dt }
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

    #[test]
    fn runtime_config_defaults_match_legacy_hardcoded_values() {
        // 这两个值就是当年 WorldConfig::default() 里硬编码的那一对 ——
        // 改默认值会让所有旧项目的物理行为悄悄变样。
        let rt = RuntimeConfig::default();
        assert_eq!(rt.gravity, [0.0, -9.81, 0.0]);
        assert_eq!(rt.fixed_dt, 1.0 / 60.0);
    }

    #[test]
    fn runtime_config_sanitized_rejects_garbage() {
        // 固定步长 0 = 累加器永远凑不满一步 = 时间静止，必须退回默认。
        let zero = RuntimeConfig { fixed_dt: 0.0, ..Default::default() }.sanitized();
        assert_eq!(zero.fixed_dt, 1.0 / 60.0);

        let nan = RuntimeConfig { fixed_dt: f32::NAN, ..Default::default() }.sanitized();
        assert_eq!(nan.fixed_dt, 1.0 / 60.0);

        let wild = RuntimeConfig { gravity: [f32::NAN, -1e30, 0.0], ..Default::default() }.sanitized();
        assert_eq!(wild.gravity, RuntimeConfig::default().gravity);

        // 合法但过慢的步长要夹住（10Hz 物理肉眼可见地卡）。
        let slow = RuntimeConfig { fixed_dt: 0.5, ..Default::default() }.sanitized();
        assert_eq!(slow.fixed_dt, 1.0 / 10.0);
    }
}
