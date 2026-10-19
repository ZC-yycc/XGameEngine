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
// 画面渲染参数
// ---------------------------------------------------------------------------

/// 色调映射算子。
///
/// 枚举而不是 bool（"开 / 关"）：色调映射是一整类算子的位置 —— Reinhard /
/// Filmic / Neutral 随时可以加进来，而 bool 到时候还要改磁盘格式。当前只有
/// 两个变体，`Aces` 是默认。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToneMapMode {
    /// 电影级 ACES 拟合曲线（RRT + ODT 的有理拟合）：高光滚降不裁剪，
    /// 颜色先转到 ACEScg（AP1）再回来，过曝处保留色相。默认。
    #[default]
    Aces,
    /// 不做色调映射：只有曝光与钳制 —— 高光直接裁到纯白。
    ///
    /// 保留它是刻意的：它是 ACES 上线前的旧画面（对照基线），也是
    /// "我要 flat 画面"的逃生舱；`scene_signature` 与报告都认这个开关。
    Off,
}

/// Bloom（泛光）参数：亮部溢出的柔光。
///
/// 三个字段就是"经典三件套"：**提取**（threshold / knee）控制"多亮才算亮"，
/// **强度**（intensity）控制光晕比例。`intensity = 0` 即关闭（不设独立开关：
/// 两个字段表达同一个状态迟早互相矛盾），关闭时渲染器**整段跳过** ——
/// 不建纹理、不跑 pass，是真正的零成本。
///
/// 为什么默认关：Bloom 是**风格**参数（写实档也常关），不是"正确性"参数 ——
/// 与色调映射（默认 ACES，因为它回答"高光怎么落到显示区间"这个必须回答的
/// 问题）不同。要发光的项目把 intensity 调到 0.4~0.8 即可。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BloomConfig {
    /// 光晕强度（叠加比例，0 = 关闭）。
    pub intensity: f32,
    /// 亮部提取阈值（曝光后的线性亮度；1.0 ≈ 显示白点）。
    pub threshold: f32,
    /// 软膝半宽：阈值附近多宽的一段"淡入"，避免亮斑边缘出现硬边。
    pub knee: f32,
}

impl Default for BloomConfig {
    fn default() -> Self {
        Self {
            intensity: 0.0,
            threshold: 1.0,
            knee: 0.6,
        }
    }
}

impl BloomConfig {
    /// 光晕强度的上限：再亮只是整幅糊成一片白，没有可用的画面。
    pub const MAX_INTENSITY: f32 = 10.0;
    /// 阈值上限：场景亮度标定是"白面 ≈ 1.0"，100 已远超任何真实内容。
    pub const MAX_THRESHOLD: f32 = 100.0;
    /// 软膝半宽上限（线性亮度单位）。
    pub const MAX_KNEE: f32 = 4.0;

    /// 与默认值完全一致时不落盘（`skip_serializing_if` 用）：默认清单里不出现
    /// 这一节，与"没有调过画面的项目"保持逐字节一致的 diff 噪声水平。
    /// 注意判据是**整体相等**而不是"intensity == 0"：关了光晕但调过阈值的
    /// 配置照样落盘 —— 用户调过的参数不悄悄丢。
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// 容错读取（同 [`RenderConfig::sanitized`] 的契约）：
    /// NaN / 无穷 / 负数落回默认值，合法但离谱的量级夹进安全范围。
    pub fn sanitized(self) -> Self {
        let intensity = if self.intensity.is_finite() {
            self.intensity.clamp(0.0, Self::MAX_INTENSITY)
        } else {
            Self::default().intensity
        };
        let threshold = if self.threshold.is_finite() {
            self.threshold.clamp(0.0, Self::MAX_THRESHOLD)
        } else {
            Self::default().threshold
        };
        let knee = if self.knee.is_finite() {
            self.knee.clamp(0.0, Self::MAX_KNEE)
        } else {
            Self::default().knee
        };
        Self {
            intensity,
            threshold,
            knee,
        }
    }
}

/// 画面渲染参数（色调映射 / 曝光 / 泛光），持久化在 `project.ron` 的 `render` 节。
///
/// 与 [`RuntimeConfig`] 同一套路：此前曝光是硬编码常量、色调映射根本不存在；
/// 现在它们是**数据** —— 编辑器 ⚙ Project Settings 可调、智能体改一行文本即可、
/// 编辑器 Play / 独立游戏 / 无头截图三端走同一份配置（画面一致性的单入口）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RenderConfig {
    /// 曝光倍数（线性光乘子，1.0 = 不增不减）。
    pub exposure: f32,
    /// 色调映射算子，见 [`ToneMapMode`]。
    pub tone_map: ToneMapMode,
    /// 泛光参数，见 [`BloomConfig`]。
    ///
    /// 后加的字段：`serde(default)` 让旧清单照常加载；关着时不落盘
    /// （老项目 diff 不出新节的噪声）。
    #[serde(default, skip_serializing_if = "BloomConfig::is_default")]
    pub bloom: BloomConfig,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            exposure: 1.0,
            tone_map: ToneMapMode::Aces,
            bloom: BloomConfig::default(),
        }
    }
}

impl RenderConfig {
    /// 曝光的安全范围。
    ///
    /// 下界不是 0：曝光 0 = 全黑画面（不是"关掉曝光"，只有 `tone_map` 才有
    /// 关的概念）；上界 20 已足够"夜里看太阳"，再大只是整幅纯白。
    pub const MIN_EXPOSURE: f32 = 0.05;
    pub const MAX_EXPOSURE: f32 = 20.0;

    /// 容错读取：清单是**手改的文本**，非法值不能直接交给渲染器。
    ///
    /// - 曝光 NaN / 无穷 / 非正 → 退回默认 1.0；
    /// - 合法但离谱的量级夹进 [`Self::MIN_EXPOSURE`] / [`Self::MAX_EXPOSURE`]；
    /// - Bloom 三件套走 [`BloomConfig::sanitized`]。
    pub fn sanitized(self) -> Self {
        let exposure = if self.exposure.is_finite() && self.exposure > 0.0 {
            self.exposure.clamp(Self::MIN_EXPOSURE, Self::MAX_EXPOSURE)
        } else {
            Self::default().exposure
        };
        Self {
            exposure,
            tone_map: self.tone_map,
            bloom: self.bloom.sanitized(),
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

    /// 渲染参数默认值 = ACES + 曝光 1.0。
    ///
    /// 这条钉死两件事：① 新场景默认就是色调映射后的画面（ACES 不是
    /// 可选项而是基线）；② 曝光 1.0 是"不增不减"，历史场景的亮度标定
    /// （`radiance = intensity × π`）不被悄悄改档。
    #[test]
    fn render_config_defaults_to_aces_and_unit_exposure() {
        let r = RenderConfig::default();
        assert_eq!(r.tone_map, ToneMapMode::Aces);
        assert_eq!(r.exposure, 1.0);
    }

    /// 曝光 0 / 负数 / NaN / 无穷必须退回默认（全黑画面不合法），
    /// 合法但离谱的量级（0.001 / 1e6）夹进安全范围。
    #[test]
    fn render_config_sanitized_rejects_garbage() {
        let zero = RenderConfig { exposure: 0.0, ..Default::default() }.sanitized();
        assert_eq!(zero.exposure, 1.0, "曝光 0 = 全黑，必须退回默认");

        let negative = RenderConfig { exposure: -3.0, ..Default::default() }.sanitized();
        assert_eq!(negative.exposure, 1.0);

        let nan = RenderConfig { exposure: f32::NAN, ..Default::default() }.sanitized();
        assert_eq!(nan.exposure, 1.0);

        let inf = RenderConfig { exposure: f32::INFINITY, ..Default::default() }.sanitized();
        assert_eq!(inf.exposure, 1.0);

        let tiny = RenderConfig { exposure: 0.0001, ..Default::default() }.sanitized();
        assert_eq!(tiny.exposure, RenderConfig::MIN_EXPOSURE);

        let huge = RenderConfig { exposure: 1e9, ..Default::default() }.sanitized();
        assert_eq!(huge.exposure, RenderConfig::MAX_EXPOSURE);

        // 合法值原样保留，色调映射开关不受曝光夹取影响。
        let ok = RenderConfig {
            exposure: 2.5,
            tone_map: ToneMapMode::Off,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(ok.exposure, 2.5);
        assert_eq!(ok.tone_map, ToneMapMode::Off);
    }

    /// Bloom 默认必须是**关**（intensity 0）：它是风格参数，默认改变画面
    /// 是不可接受的静默行为；要发光由用户显式调 intensity。
    #[test]
    fn bloom_defaults_to_off() {
        let b = BloomConfig::default();
        assert_eq!(b.intensity, 0.0, "Bloom 默认必须关闭");
        assert!(b.is_default());

        // 默认配置序列化后不出现 bloom 节（没调过画面的项目 diff 干净）。
        let text = ron::to_string(&RenderConfig::default()).expect("serialize");
        assert!(!text.contains("bloom"), "未调过的 Bloom 不该落盘: {text}");

        // 关了光晕但调过阈值 → 照样落盘（用户调过的参数不悄悄丢）。
        let tuned = RenderConfig {
            bloom: BloomConfig {
                threshold: 2.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let text = ron::to_string(&tuned).expect("serialize");
        assert!(text.contains("bloom"), "调过的参数必须落盘: {text}");
        let back: RenderConfig = ron::from_str(&text).expect("deserialize");
        assert_eq!(back, tuned);
    }

    /// Bloom 容错：NaN / 负数 / 超限。intensity 负数 = 关（不是"反向光晕"）。
    #[test]
    fn bloom_config_sanitized_rejects_garbage() {
        let nan = BloomConfig {
            intensity: f32::NAN,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(nan.intensity, 0.0, "NaN 强度必须落回默认（关）");

        let negative = BloomConfig {
            intensity: -2.0,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(negative.intensity, 0.0, "负强度 = 关，不是反向光晕");

        let huge = BloomConfig {
            intensity: 1e6,
            threshold: -5.0,
            knee: f32::INFINITY,
        }
        .sanitized();
        assert_eq!(huge.intensity, BloomConfig::MAX_INTENSITY);
        assert_eq!(huge.threshold, 0.0, "负阈值夹到 0（全亮部都提取）");
        assert_eq!(huge.knee, BloomConfig::default().knee, "无穷膝宽落回默认");

        let ok = BloomConfig {
            intensity: 0.6,
            threshold: 1.2,
            knee: 0.5,
        }
        .sanitized();
        assert_eq!(ok.intensity, 0.6);
        assert_eq!(ok.threshold, 1.2);
        assert_eq!(ok.knee, 0.5);
    }
}
