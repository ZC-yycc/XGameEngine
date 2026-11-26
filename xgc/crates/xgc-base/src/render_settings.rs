//! 项目级画面设置供迁移、打包与各宿主共用，不持有渲染资源。

use serde::{Deserialize, Serialize};

/// 渲染器可选的色调映射算子。关闭时仍应用曝光并钳制高光。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToneMapMode {
    #[default]
    Aces,
    Off,
}

/// 项目作者指定的画面参数，独立于硬件档位。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderSettings {
    pub exposure: f32,
    pub tone_map: ToneMapMode,
    pub lod_bias: f32,
    pub sky: SkySettings,
    pub bloom: BloomSettings,
    pub fxaa: FxaaSettings,
    pub shadow: ShadowSettings,
    pub taa: TaaSettings,
    pub ssao: SsaoSettings,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            exposure: EXPOSURE_DEFAULT,
            tone_map: ToneMapMode::default(),
            lod_bias: 1.0,
            sky: SkySettings::default(),
            bloom: BloomSettings::default(),
            fxaa: FxaaSettings::default(),
            shadow: ShadowSettings::default(),
            taa: TaaSettings::default(),
            ssao: SsaoSettings::default(),
        }
    }
}

impl RenderSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn sanitized(self) -> Self {
        Self {
            exposure: sanitize_exposure(self.exposure),
            tone_map: self.tone_map,
            lod_bias: if self.lod_bias.is_finite() {
                self.lod_bias
            } else {
                1.0
            },
            sky: self.sky.sanitized(),
            bloom: self.bloom.sanitized(),
            fxaa: self.fxaa,
            shadow: self.shadow.sanitized(),
            taa: self.taa.sanitized(),
            ssao: self.ssao.sanitized(),
        }
    }
}

pub const DEFAULT_SHADOW_RESOLUTION: u32 = 2048;
const MIN_SHADOW_RESOLUTION: u32 = 256;
const MAX_SHADOW_RESOLUTION: u32 = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum QualityPreset {
    Low,
    Medium,
    High,
}

/// 显式覆盖优先于档位，没写档位时跟设备能力。
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderQuality {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<QualityPreset>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub msaa: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadow_resolution: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lod_scale: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedQuality {
    pub msaa: u32,
    pub shadow_resolution: u32,
    pub lod_scale: f32,
}

impl RenderQuality {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn sanitized(mut self) -> Self {
        self.lod_scale = self.lod_scale.filter(|value| value.is_finite());
        self
    }

    pub fn resolve(self, device_msaa: u32) -> ResolvedQuality {
        let (msaa, shadow_resolution, lod_scale) = match self.preset {
            None => (device_msaa, DEFAULT_SHADOW_RESOLUTION, 1.0),
            Some(QualityPreset::Low) => (1, 1024, 0.6),
            Some(QualityPreset::Medium) => (4, DEFAULT_SHADOW_RESOLUTION, 1.0),
            Some(QualityPreset::High) => (4, 4096, 1.25),
        };
        ResolvedQuality {
            msaa: if self.msaa.unwrap_or(msaa).min(device_msaa) >= 4 {
                4
            } else {
                1
            },
            shadow_resolution: self
                .shadow_resolution
                .unwrap_or(shadow_resolution)
                .clamp(MIN_SHADOW_RESOLUTION, MAX_SHADOW_RESOLUTION),
            lod_scale: self
                .sanitized()
                .lod_scale
                .unwrap_or(lod_scale)
                .clamp(0.05, 10.0),
        }
    }
}

/// 灯具优先于档位。点光立方体有六个面，独立限制纹理边长以控制显存。
pub fn shadow_map_sizes(default: u32, sun: u32, point: u32, device_limit: u32) -> [u32; 2] {
    let sun = if sun == 0 { default } else { sun };
    let point = if point == 0 {
        (default / 2).clamp(MIN_SHADOW_RESOLUTION, DEFAULT_SHADOW_RESOLUTION / 2)
    } else {
        point.clamp(64, 2048)
    };
    [sun.min(device_limit), point.min(device_limit)]
}

#[cfg(test)]
mod quality_tests {
    use super::*;

    #[test]
    fn applies_supported_msaa_and_sanitizes_overrides() {
        assert_eq!(RenderQuality::default().resolve(4).msaa, 4);
        assert_eq!(RenderQuality::default().resolve(1).msaa, 1);
        let high = RenderQuality {
            preset: Some(QualityPreset::High),
            msaa: Some(2),
            shadow_resolution: Some(u32::MAX),
            lod_scale: Some(f32::NAN),
        };
        assert_eq!(
            high.resolve(4),
            ResolvedQuality {
                msaa: 1,
                shadow_resolution: 8192,
                lod_scale: 1.25,
            }
        );
        let medium = RenderQuality {
            preset: Some(QualityPreset::Medium),
            ..Default::default()
        };
        assert_eq!(medium.resolve(1).msaa, 1);
    }

    #[test]
    fn light_resolution_precedes_project_default_and_fits_device() {
        assert_eq!(shadow_map_sizes(1024, 0, 0, 8192), [1024, 512]);
        assert_eq!(shadow_map_sizes(4096, 768, 32, 8192), [768, 64]);
        assert_eq!(shadow_map_sizes(4096, 8192, 8192, 1024), [1024, 1024]);
    }
}

/// 缺省曝光（不增不减）。
pub const EXPOSURE_DEFAULT: f32 = 1.0;

/// 曝光下限：0 会让画面全黑，不是"关闭曝光"的形态。
pub const MIN_EXPOSURE: f32 = 0.05;

/// 曝光上限：再大只会把整幅画推到纯白。
pub const MAX_EXPOSURE: f32 = 20.0;

/// 按旧端 `RenderConfig::sanitized` 的口径消毒一个曝光值。
///
/// 清单是手改的文本：`NaN` / 无穷 / 非正值直接落回缺省（`f32::clamp` 对 NaN
/// 原样返回，不能靠它兜）；合法但越界的量级再夹进安全范围。
pub fn sanitize_exposure(value: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value.clamp(MIN_EXPOSURE, MAX_EXPOSURE)
    } else {
        EXPOSURE_DEFAULT
    }
}

#[cfg(test)]
mod exposure_tests {
    use super::{sanitize_exposure, EXPOSURE_DEFAULT, MAX_EXPOSURE, MIN_EXPOSURE};

    #[test]
    fn default_leaves_the_image_untouched() {
        assert_eq!(EXPOSURE_DEFAULT, 1.0);
        assert_eq!(sanitize_exposure(1.0), 1.0);
    }

    #[test]
    fn non_finite_and_non_positive_values_fall_back_to_the_default() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, -3.0] {
            assert_eq!(
                sanitize_exposure(value),
                EXPOSURE_DEFAULT,
                "曝光 {value} 必须落回缺省（0 = 全黑，不是关闭态）"
            );
        }
    }

    #[test]
    fn legal_values_are_clamped_into_the_safe_range() {
        assert_eq!(sanitize_exposure(0.0001), MIN_EXPOSURE);
        assert_eq!(sanitize_exposure(1e9), MAX_EXPOSURE);
        assert_eq!(sanitize_exposure(2.5), 2.5);
    }
}

/// 光晕参数：与旧端 `BloomConfig` 逐字段、逐缺省值一致。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BloomSettings {
    /// 光晕强度（叠加比例，0 = 关闭）。
    pub intensity: f32,
    /// 亮部提取阈值（曝光后的线性亮度；1.0 ≈ 显示白点）。
    pub threshold: f32,
    /// 软膝半宽：阈值附近多宽的一段"淡入"，避免亮斑边缘出现硬边。
    pub knee: f32,
}

impl Default for BloomSettings {
    fn default() -> Self {
        Self {
            intensity: 0.0,
            threshold: 1.0,
            knee: 0.6,
        }
    }
}

impl BloomSettings {
    /// 光晕强度的上限：再亮只是整幅糊成一片白，没有可用的画面。
    pub const MAX_INTENSITY: f32 = 10.0;
    /// 阈值上限：场景亮度标定是"白面 ≈ 1.0"，100 已远超任何真实内容。
    pub const MAX_THRESHOLD: f32 = 100.0;
    /// 软膝半宽上限（线性亮度单位）。
    pub const MAX_KNEE: f32 = 4.0;

    /// 容错读取（同旧端 `BloomConfig::sanitized` 的契约）：NaN / 无穷 / 负数
    /// 落回默认值，合法但离谱的量级夹进安全范围。
    pub fn sanitized(self) -> Self {
        let fallback = Self::default();
        let clamp = |value: f32, max: f32, default: f32| {
            if value.is_finite() {
                value.clamp(0.0, max)
            } else {
                default
            }
        };
        Self {
            intensity: clamp(self.intensity, Self::MAX_INTENSITY, fallback.intensity),
            threshold: clamp(self.threshold, Self::MAX_THRESHOLD, fallback.threshold),
            knee: clamp(self.knee, Self::MAX_KNEE, fallback.knee),
        }
    }

    /// `intensity == 0` 即关闭：不设独立开关，两个字段表达同一状态迟早互相矛盾。
    pub fn is_enabled(&self) -> bool {
        self.intensity > 0.0
    }
}

/// 画面设置里的 FXAA 节：逐项对应旧端 `render.fxaa.enabled`。
///
/// 默认关：项目可以省略这一节。
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FxaaSettings {
    /// 是否在 tone-map 之后跑一趟 FXAA。
    pub enabled: bool,
}

impl FxaaSettings {
    /// 这一帧要不要真的跑 FXAA。
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}

/// 历史权重上限。1.0 会把历史冻住（画面再也不更新），旧端同样在 0.98 封顶。
pub const MAX_HISTORY_WEIGHT: f32 = 0.98;
/// 历史权重的缺省值：本帧颜色只占 10%。
pub const DEFAULT_HISTORY_WEIGHT: f32 = 0.9;

/// TAA 设置：与旧端 `TaaConfig` 逐字段、逐缺省值一致（落盘位置 `render.taa`）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TaaSettings {
    /// 总开关。缺省关：关着时整段跳过，画面逐位不变。
    pub enabled: bool,
    /// 历史权重（0 = 完全不用历史，0.98 = 几乎冻结）；只在已有可信上一帧时生效。
    pub history_weight: f32,
}

impl Default for TaaSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            history_weight: DEFAULT_HISTORY_WEIGHT,
        }
    }
}

impl TaaSettings {
    /// 容错读取（同旧端 `TaaConfig::sanitized`）：非有限权重落回缺省值，
    /// 合法值夹进 `[0, 0.98]`。
    pub fn sanitized(self) -> Self {
        let history_weight = if self.history_weight.is_finite() {
            self.history_weight.clamp(0.0, MAX_HISTORY_WEIGHT)
        } else {
            DEFAULT_HISTORY_WEIGHT
        };
        Self {
            enabled: self.enabled,
            history_weight,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// 天空整体强度上限：再亮整幅背景糊成纯白。
pub const MAX_INTENSITY: f32 = 10.0;
/// 太阳强度上限。
pub const MAX_SUN_INTENSITY: f32 = 10.0;
/// 太阳盘半径范围（度）。
pub const MIN_SUN_SIZE: f32 = 0.1;
pub const MAX_SUN_SIZE: f32 = 12.0;

/// 画面设置里的天空节（对应旧 `project.ron` 的 `render.sky`）。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SkySettings {
    pub enabled: bool,
    /// 整体强度乘子（0 = 天空全黑，等价于只有 ambient）。
    pub intensity: f32,
    pub zenith: [f32; 3],
    /// 天空段与地面段在视线水平处**都**取它 —— 渐变在 `dir.y == 0` 处连续。
    pub horizon: [f32; 3],
    /// 镜头朝下看时的颜色。
    pub ground: [f32; 3],
    /// 太阳盘半径（度）。
    pub sun_size: f32,
    /// 太阳盘与光晕强度乘子；场景没有方向光时太阳整段不出现。
    pub sun_intensity: f32,
}

impl Default for SkySettings {
    /// 与旧 `SkyConfig::default` 一致：默认关、强度 1。关着时环境照明只剩 `ambient`，
    /// 画面与天空上线前逐位相同。
    fn default() -> Self {
        Self {
            enabled: false,
            intensity: 1.0,
            zenith: [0.12, 0.28, 0.58],
            horizon: [0.58, 0.68, 0.82],
            ground: [0.16, 0.15, 0.14],
            sun_size: 1.5,
            sun_intensity: 1.0,
        }
    }
}

impl SkySettings {
    /// 加载边界清洗：NaN / 无穷换缺省值，强度与太阳半径夹到安全范围。
    pub fn sanitized(mut self) -> Self {
        for color in [&mut self.zenith, &mut self.horizon, &mut self.ground] {
            for channel in color.iter_mut() {
                if !channel.is_finite() {
                    *channel = 0.0;
                }
            }
        }
        if !self.intensity.is_finite() {
            self.intensity = 1.0;
        }
        if !self.sun_intensity.is_finite() {
            self.sun_intensity = 1.0;
        }
        if !self.sun_size.is_finite() {
            self.sun_size = 1.5;
        }
        self.intensity = self.intensity.clamp(0.0, MAX_INTENSITY);
        self.sun_intensity = self.sun_intensity.clamp(0.0, MAX_SUN_INTENSITY);
        self.sun_size = self.sun_size.clamp(MIN_SUN_SIZE, MAX_SUN_SIZE);
        self
    }
}

/// 画面设置里的 SSAO 节：默认关，与 Bloom / TAA / FXAA 同一条纪律。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SsaoSettings {
    /// 是否启用。关着时渲染器整段跳过，画面逐位不变。
    pub enabled: bool,
    /// 遮蔽强度（0 = 无遮蔽效果；上限见 [`Self::MAX_STRENGTH`]）。
    pub strength: f32,
    /// 采样半径（世界单位）。太大 = 远处物体也遮蔽近处；太小 = 看不出。
    pub radius: f32,
    /// 深度比较偏置（世界单位）：挡掉"表面遮蔽自己"的 acne。
    pub bias: f32,
}

impl Default for SsaoSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            strength: 1.0,
            radius: 0.5,
            bias: 0.025,
        }
    }
}

impl SsaoSettings {
    /// 遮蔽强度上限：再大只是把接触阴影压成死黑，画面信息全丢。
    pub const MAX_STRENGTH: f32 = 4.0;
    /// 采样半径上限（世界单位）：10 米的遮蔽半径已经不是"接触阴影"了。
    pub const MAX_RADIUS: f32 = 10.0;
    /// 偏置上限：偏置比半径还大时遮蔽整体失效，但那也是合法的"关"法。
    pub const MAX_BIAS: f32 = 1.0;

    /// 开着且强度为正才算数：强度 0 与关掉等价（省下四趟全屏 pass）。
    pub fn is_enabled(&self) -> bool {
        self.enabled && self.strength > 0.0
    }

    /// 与默认值逐项比较，供回归与清单写入使用。
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// 容错读取：非有限字段落回默认，合法但离谱的量级夹进安全范围（旧端
    /// `SsaoConfig::sanitized` 同式）。
    pub fn sanitized(self) -> Self {
        let clean = |value: f32, default: f32, max: f32| {
            if value.is_finite() {
                value.clamp(0.0, max)
            } else {
                default
            }
        };
        let default = Self::default();
        Self {
            enabled: self.enabled,
            strength: clean(self.strength, default.strength, Self::MAX_STRENGTH),
            radius: clean(self.radius, default.radius, Self::MAX_RADIUS),
            bias: clean(self.bias, default.bias, Self::MAX_BIAS),
        }
    }
}

/// 阴影画质：软阴影尺寸 + 级间混合带宽。
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShadowSettings {
    /// 软阴影（PCSS）的灯光相对尺寸。
    ///
    /// `0.0`（默认）= 硬阴影：固定 3×3 PCF，与引入 PCSS 之前逐位一致；`> 0` =
    /// 先在盘里找遮挡物估出半影宽度，再按宽度做软化 PCF —— 贴着遮挡物的影子硬、
    /// 离得远的影子糊，这是固定核做不到的。
    pub softness: f32,
    /// 级间混合带宽（占该级远端切分距离的比例）。`0.0`（默认）= 两级硬接；
    /// `> 0` 时只改切分前一条薄带里的亮度过渡，把缝抹平。
    pub cascade_blend: f32,
}

impl ShadowSettings {
    /// 与默认值整体相等 = 关（旧端 `ShadowConfig::is_default` 同口径）。
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// 是否有任何一项开着：决定着色器走新路径还是老的硬阴影路径。
    pub fn is_enabled(&self) -> bool {
        self.softness > 0.0 || self.cascade_blend > 0.0
    }

    /// 容错读取：`project.ron` 是**手改的文本**，负数 / NaN / 无穷不能直接交给
    /// 着色器（负半径会让 PCF 的采样点绕到反方向，NaN 会让整片影子消失）。
    ///
    /// 非有限值与负值一律当 0（= 该子项关），**不夹上界** —— 上界没有物理意义，
    /// 太大了只会糊成一片，不该由引擎替作者决定（与旧端 `ShadowConfig::sanitized`
    /// 逐项同口径）。
    pub fn sanitized(self) -> Self {
        let clean = |value: f32| {
            if value.is_finite() && value > 0.0 {
                value
            } else {
                0.0
            }
        };
        Self {
            softness: clean(self.softness),
            cascade_blend: clean(self.cascade_blend),
        }
    }
}

#[cfg(test)]
mod shadow_tests {
    use super::*;

    #[test]
    fn defaults_are_off() {
        let settings = ShadowSettings::default();
        assert_eq!(settings.softness, 0.0);
        assert_eq!(settings.cascade_blend, 0.0);
        assert!(settings.is_default());
        assert!(!settings.is_enabled());
    }

    #[test]
    fn illegal_values_fall_back_to_off() {
        // 旧端 `ShadowConfig::sanitized` 的单测逐条对过来：NaN / 无穷 / 负数
        // 都必须是"关"，而不是把 NaN 灌进着色器的盘半径。
        let garbage = ShadowSettings {
            softness: f32::NAN,
            cascade_blend: f32::NEG_INFINITY,
        }
        .sanitized();
        assert_eq!(garbage, ShadowSettings::default());
        let negative = ShadowSettings {
            softness: -0.5,
            cascade_blend: -0.1,
        }
        .sanitized();
        assert_eq!(negative, ShadowSettings::default());
        assert!(!negative.is_enabled());
    }

    #[test]
    fn legal_values_pass_through_without_an_upper_clamp() {
        let settings = ShadowSettings {
            softness: 0.5,
            cascade_blend: 0.05,
        }
        .sanitized();
        assert_eq!(settings.softness, 0.5);
        assert_eq!(settings.cascade_blend, 0.05);
        assert!(!settings.is_default());
        assert!(settings.is_enabled());
        // 上界刻意不夹：旧端认为"糊成一片"是作者的自由。
        assert_eq!(
            ShadowSettings {
                softness: 12.0,
                cascade_blend: 3.0,
            }
            .sanitized()
            .softness,
            12.0
        );
    }
}
