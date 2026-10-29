//! 质量档位（低 / 中 / 高）：把"这台机器要多好的画面"映射成一组渲染参数。
//!
//! # 为什么要有这一层
//!
//! 差距分析 P2 #23 的原话：「MSAA 取适配器档位、阴影分辨率按灯设置，无低 / 中 /
//! 高档位预设」。结果是作者想要"低配机器能跑"时没有任何抓手 —— 采样数永远是
//! 适配器最大、阴影贴图永远 2048，只能逐项手改、还改不动（采样数在管线创建期
//! 就定死了）。档位把这组"机器向"的取舍收成一个词。
//!
//! # 只管"机器向"的旋钮，不碰作者的画面风格
//!
//! 曝光 / 色调映射 / 天空 / Bloom 是**内容**（项目长什么样），不是画质档位；
//! FXAA / TAA 是项目级 AA 决策（`render.fxaa` / `render.taa`），同理。档位只管：
//!
//! | 旋钮 | 低 | 中 | 高 | 没配 `quality` 节时 |
//! |---|---|---|---|---|
//! | MSAA 采样数 | 1× | 4× | 4× | 适配器最大 |
//! | 阴影贴图默认边长 | 1024 | 2048 | 4096 | 2048 |
//! | LOD 阈值缩放 | 0.6 | 1.0 | 1.25 | 1.0 |
//!
//! **采样数只有 1× 与 4×**：wgpu 对可渲染格式只保证这两档（2× / 8× 要
//! 适配器私有特性，实测 RTX 2060 上 `Rgba16Float` 开 2× 直接
//! "Sample count 2 is not supported"）。中档 = 今天的行为，高档只多阴影精度。
//!
//! **没配 `quality` 节 = 逐位与引入本模块之前一致**（最后一列）。
//!
//! # 两条既有旋钮的关系
//!
//! - `Light::shadow_resolution != 0` 时**灯说了算**（作者逐灯精调），
//!   档位只决定"灯没写时用什么"；
//! - LOD 最终阈值 = `max_distance × render.lod_bias × quality.lod_scale` ——
//!   `lod_bias` 是**内容**旋钮（作者说"我的场景在这台机器上该多远切"），
//!   `lod_scale` 是**档位**缩放（低配整体拉近）。两者相乘而不是互相覆盖，
//!   任何一边单独改动都有可预期的效果。

use serde::{Deserialize, Serialize};

/// 阴影贴图的默认边长（灯没写 `shadow_resolution` 时用它）。
///
/// 与 `xgame_render::scene_pipeline` 的常量是**同一个数**（那边直接 re-export，
/// 别再抄一份 2048）。
pub const DEFAULT_SHADOW_RESOLUTION: u32 = 2048;

/// 阴影贴图边长的允许范围（手改 `project.ron` 的兜底）。
pub const MIN_SHADOW_RESOLUTION: u32 = 256;
pub const MAX_SHADOW_RESOLUTION: u32 = 8192;

/// LOD 缩放的允许范围（`<= 0` 的"关掉 LOD"留给 `render.lod_bias`）。
pub const MIN_LOD_SCALE: f32 = 0.05;
pub const MAX_LOD_SCALE: f32 = 10.0;

/// 质量档位。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum QualityPreset {
    /// 低配：无 MSAA、阴影 1024、LOD 提前切（阈值 × 0.6）。
    Low,
    /// 中配：2× MSAA、阴影 2048、LOD 原阈值。
    #[default]
    Medium,
    /// 高配：4× MSAA、阴影 4096、LOD 推迟切换（阈值 × 1.25）。
    High,
}

impl QualityPreset {
    /// 全部档位（下拉 / 测试用）。
    pub const ALL: [QualityPreset; 3] = [QualityPreset::Low, QualityPreset::Medium, QualityPreset::High];

    /// 脚本 / CLI 里的名字（纯 ASCII 小写）。
    pub fn name(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    /// 按名字解析（大小写不敏感）。
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "low" => Some(Self::Low),
            "medium" | "med" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }

    /// 档位的"出厂"参数（`msaa` **未**与适配器取 min，见
    /// [`QualityConfig::resolve`]）。
    pub fn base(self) -> QualitySettings {
        match self {
            Self::Low => QualitySettings {
                msaa: 1,
                shadow_resolution: 1024,
                lod_scale: 0.6,
            },
            // 中档 = "今天的行为"（4× / 2048 / 1.0）：档位的基准线。
            Self::Medium => QualitySettings {
                msaa: 4,
                shadow_resolution: DEFAULT_SHADOW_RESOLUTION,
                lod_scale: 1.0,
            },
            Self::High => QualitySettings {
                msaa: 4,
                shadow_resolution: 4096,
                lod_scale: 1.25,
            },
        }
    }
}

/// 一组已解析、已夹取的档位参数（宿主直接消费的东西）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QualitySettings {
    /// 主 pass 采样数（1 / 2 / 4 / 8，已与适配器能力取 min）。
    pub msaa: u32,
    /// 阴影贴图默认边长（灯自己写了就用灯的）。
    pub shadow_resolution: u32,
    /// LOD 阈值的整体缩放（与 `render.lod_bias` **相乘**）。
    pub lod_scale: f32,
}

impl QualitySettings {
    /// "没配 `quality` 节"时的行为：适配器最大采样 / [`DEFAULT_SHADOW_RESOLUTION`] / 1.0。
    pub fn reference(adapter_max_msaa: u32) -> Self {
        Self {
            msaa: snap_msaa(adapter_max_msaa, adapter_max_msaa),
            shadow_resolution: DEFAULT_SHADOW_RESOLUTION,
            lod_scale: 1.0,
        }
    }
}

/// 项目清单里的 `quality` 节。
///
/// **全空 = 不存在**（`skip_serializing_if`）：老项目不写这一节，行为与引入
/// 档位之前逐位一致。想要哪一项跟着档位走就配 `preset`，想单独拧一颗螺丝就写
/// 对应的覆盖字段（覆盖优先于档位）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct QualityConfig {
    /// 档位。`None` = 以"没配这一节"的行为为底，只应用下面的显式覆盖。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<QualityPreset>,
    /// 显式覆盖 MSAA 采样数（会被夹到 `1..=8` 的 2 的幂，并与适配器取 min）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msaa: Option<u32>,
    /// 显式覆盖阴影贴图默认边长（会被夹到 [`MIN_SHADOW_RESOLUTION`]..=[`MAX_SHADOW_RESOLUTION`]）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow_resolution: Option<u32>,
    /// 显式覆盖 LOD 缩放（会被夹到 [`MIN_LOD_SCALE`]..=[`MAX_LOD_SCALE`]）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lod_scale: Option<f32>,
}

impl QualityConfig {
    /// 全空（= 不存在）。落盘时整段省略。
    pub fn is_empty(&self) -> bool {
        self.preset.is_none()
            && self.msaa.is_none()
            && self.shadow_resolution.is_none()
            && self.lod_scale.is_none()
    }

    /// 解析成宿主要消费的参数：档位为底、显式覆盖优先、最后统一夹取。
    ///
    /// `adapter_max_msaa` 传 `xgame_render::scene_pipeline::supported_sample_count`
    /// 的结果（1 或 4）—— 采样数是**格式相关**的能力，必须按 HDR 格式问；
    /// 2× / 8× 不是 wgpu 保证档，见 [`snap_msaa`]。
    pub fn resolve(&self, adapter_max_msaa: u32) -> QualitySettings {
        let base = self
            .preset
            .map_or_else(|| QualitySettings::reference(adapter_max_msaa), |p| p.base());
        QualitySettings {
            msaa: snap_msaa(self.msaa.unwrap_or(base.msaa), adapter_max_msaa),
            shadow_resolution: self
                .shadow_resolution
                .unwrap_or(base.shadow_resolution)
                .clamp(MIN_SHADOW_RESOLUTION, MAX_SHADOW_RESOLUTION),
            lod_scale: finite_or(self.lod_scale, base.lod_scale).clamp(MIN_LOD_SCALE, MAX_LOD_SCALE),
        }
    }
}

/// 坏浮点（`NaN` / `±∞`）退回基准值：`f32::clamp` 对 `NaN` 会原样返回 `NaN`，
/// 而"档位把 LOD 缩放写成 NaN"不该让渲染侧的兜底去收拾。
fn finite_or(value: Option<f32>, fallback: f32) -> f32 {
    match value {
        Some(v) if v.is_finite() => v,
        _ => fallback,
    }
}

/// 采样数**只有 1× 与 4× 两档**（wgpu 对可渲染格式只保证这两个值；2× / 8×
/// 要适配器私有特性，RTX 2060 上 `Rgba16Float` 开 2× 会让管线创建直接失败）。
/// 请求 ≥ 4 且适配器支持 4× → 4，否则 1。
fn snap_msaa(requested: u32, adapter_max_msaa: u32) -> u32 {
    if requested >= 4 && adapter_max_msaa >= 4 {
        4
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **没配 `quality` 节 = 今天的行为**：适配器最大采样 / 2048 / 1.0。
    #[test]
    fn reference_settings_match_pre_quality_behavior() {
        let settings = QualitySettings::reference(4);
        assert_eq!(settings.msaa, 4);
        assert_eq!(settings.shadow_resolution, DEFAULT_SHADOW_RESOLUTION);
        assert_eq!(settings.lod_scale, 1.0);

        // 适配器不支持 4× 的机器上，"今天的行为"就是 1×（`supported_sample_count`
        // 只会返回 1 或 4 —— 2× / 8× 不是 wgpu 保证档）。
        assert_eq!(QualitySettings::reference(1).msaa, 1);
    }

    /// 档位映射（红灯落点：`resolve` 忽略档位 / 档位表写错时这里全红）。
    ///
    /// 走 `QualityConfig::resolve` 而不是直接问 `base()` —— 真实入口是前者，
    /// 只测 `base()` 的话"解析时忘了用档位"照样绿。
    #[test]
    fn presets_map_to_their_table() {
        let via_resolve = |preset: QualityPreset| {
            QualityConfig {
                preset: Some(preset),
                ..QualityConfig::default()
            }
            .resolve(8)
        };
        let low = via_resolve(QualityPreset::Low);
        assert_eq!((low.msaa, low.shadow_resolution, low.lod_scale), (1, 1024, 0.6));
        let medium = via_resolve(QualityPreset::Medium);
        assert_eq!(
            (medium.msaa, medium.shadow_resolution, medium.lod_scale),
            (4, 2048, 1.0),
            "中档 = 今天的行为"
        );
        let high = via_resolve(QualityPreset::High);
        assert_eq!(
            (high.msaa, high.shadow_resolution, high.lod_scale),
            (4, 4096, 1.25)
        );

        // 采样数只有 1× / 4×：适配器不支持 4× 时高配也退回 1×。
        let config = QualityConfig {
            preset: Some(QualityPreset::High),
            ..QualityConfig::default()
        };
        assert_eq!(config.resolve(1).msaa, 1, "适配器不支持 4× 就只能 1×");
        assert_eq!(config.resolve(4).msaa, 4);
    }

    /// 显式覆盖优先于档位；脏值被夹取 / 吸附。
    #[test]
    fn explicit_overrides_win_and_are_clamped() {
        let config = QualityConfig {
            preset: Some(QualityPreset::High),
            msaa: Some(3),                 // → 只有 1× / 4×，3 退回 1×
            shadow_resolution: Some(9999), // → 8192
            lod_scale: Some(99.0),         // → 夹到 MAX_LOD_SCALE
        };
        let settings = config.resolve(4);
        assert_eq!(settings.msaa, 1, "3 不是支持档，退回 1×");
        assert_eq!(settings.shadow_resolution, 8192);
        assert_eq!(settings.lod_scale, MAX_LOD_SCALE);

        // 坏浮点退回档位基准（不是 NaN 一路传下去）。
        let config = QualityConfig {
            preset: Some(QualityPreset::High),
            lod_scale: Some(f32::NAN),
            ..QualityConfig::default()
        };
        assert_eq!(config.resolve(4).lod_scale, 1.25, "NaN 退回档位基准值");

        // 没有档位、只有覆盖：在"今天的行为"之上只改这一项。
        let config = QualityConfig {
            preset: None,
            msaa: Some(1),
            ..QualityConfig::default()
        };
        let settings = config.resolve(4);
        assert_eq!(settings.msaa, 1);
        assert_eq!(settings.shadow_resolution, DEFAULT_SHADOW_RESOLUTION);
        assert_eq!(settings.lod_scale, 1.0);
    }

    /// 落盘：全空不写这一节（老项目逐字节不变）；写了要能读回。
    #[test]
    fn quality_config_is_optional_in_ron() {
        assert!(QualityConfig::default().is_empty());
        let text = ron::to_string(&QualityConfig::default()).expect("序列化");
        assert_eq!(text, "()", "全空的 quality 节应序列化为空结构体");

        let config = QualityConfig {
            preset: Some(QualityPreset::Low),
            msaa: Some(1),
            shadow_resolution: None,
            lod_scale: None,
        };
        let text = ron::to_string(&config).expect("序列化");
        assert!(text.contains("Low"), "{text}");
        let back: QualityConfig = ron::from_str(&text).expect("反序列化");
        assert_eq!(back, config);
    }

    /// 档位名解析。
    #[test]
    fn preset_names_round_trip() {
        for preset in QualityPreset::ALL {
            assert_eq!(QualityPreset::from_name(preset.name()), Some(preset));
            assert_eq!(
                QualityPreset::from_name(&preset.name().to_ascii_uppercase()),
                Some(preset)
            );
        }
        assert_eq!(QualityPreset::from_name("med"), Some(QualityPreset::Medium));
        assert_eq!(QualityPreset::from_name("ultra"), None);
    }
}
