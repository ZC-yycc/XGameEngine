//! `project.ron` 的画面设置读取：曝光、天空、光晕、FXAA 与阴影画质。
//!
//! 旧引擎把画面设置放在项目级而不是场景级，XGC 也按项目级读；缺省值必须逐项
//! 等于旧端 `RenderConfig::default` 的曝光 1.0 与 `SkyConfig` / `BloomConfig` /
//! `FxaaConfig` / `ShadowConfig` 的 `default`，
//! 缺文件 / 缺节时画面才与接入前逐位相同。

use std::path::Path;

use serde::Deserialize;
use xgc_render::{sanitize_exposure, BloomSettings, FxaaSettings, ShadowSettings, SkySettings};

/// `project.ron` 的 `render` 节里 XGC 目前真正消费的部分
/// （曝光 + 天空 + 光晕 + FXAA + 阴影画质）。
#[derive(Debug, Default, Deserialize)]
struct OldProjectFile {
    #[serde(default)]
    render: OldProjectRender,
}

#[derive(Debug, Deserialize)]
struct OldProjectRender {
    #[serde(default = "exposure_default")]
    exposure: f32,
    #[serde(default)]
    sky: OldSkyConfig,
    #[serde(default)]
    bloom: OldBloomConfig,
    #[serde(default)]
    fxaa: OldFxaaConfig,
    #[serde(default)]
    shadow: OldShadowConfig,
}

/// 缺 `render` 节时的缺省：曝光必须是旧端缺省 1.0，不能靠 `derive(Default)` 给出
/// 的 0.0（那是"全黑画面"，绕开 `load_render` 直接读结构的人会踩到）。
impl Default for OldProjectRender {
    fn default() -> Self {
        Self {
            exposure: exposure_default(),
            sky: OldSkyConfig::default(),
            bloom: OldBloomConfig::default(),
            fxaa: OldFxaaConfig::default(),
            shadow: OldShadowConfig::default(),
        }
    }
}

fn exposure_default() -> f32 {
    xgc_render::EXPOSURE_DEFAULT
}

/// 缺省值必须逐项等于旧 `FxaaConfig::default`（抗锯齿默认关）。
#[derive(Debug, Default, Deserialize)]
struct OldFxaaConfig {
    #[serde(default)]
    enabled: bool,
}

/// 缺省值必须逐项等于旧 `BloomConfig::default`（光晕默认关）。
#[derive(Debug, Deserialize)]
struct OldBloomConfig {
    #[serde(default)]
    intensity: f32,
    #[serde(default = "bloom_default_threshold")]
    threshold: f32,
    #[serde(default = "bloom_default_knee")]
    knee: f32,
}

/// 缺省值必须逐项等于旧 `ShadowConfig::default`（软阴影 / 级间混合都关）。
#[derive(Debug, Default, Deserialize)]
struct OldShadowConfig {
    #[serde(default)]
    softness: f32,
    #[serde(default)]
    cascade_blend: f32,
}

fn bloom_default_threshold() -> f32 {
    1.0
}

fn bloom_default_knee() -> f32 {
    0.6
}

impl Default for OldBloomConfig {
    fn default() -> Self {
        Self {
            intensity: 0.0,
            threshold: bloom_default_threshold(),
            knee: bloom_default_knee(),
        }
    }
}

/// 缺省值必须逐项等于旧 `SkyConfig::default`（天空默认关）。
#[derive(Debug, Deserialize)]
struct OldSkyConfig {
    #[serde(default)]
    enabled: bool,
    #[serde(default = "sky_default_intensity")]
    intensity: f32,
    #[serde(default = "sky_default_zenith")]
    zenith: [f32; 3],
    #[serde(default = "sky_default_horizon")]
    horizon: [f32; 3],
    #[serde(default = "sky_default_ground")]
    ground: [f32; 3],
    #[serde(default = "sky_default_sun_size")]
    sun_size: f32,
    #[serde(default = "sky_default_intensity")]
    sun_intensity: f32,
}

fn sky_default_intensity() -> f32 {
    1.0
}

fn sky_default_zenith() -> [f32; 3] {
    [0.12, 0.28, 0.58]
}

fn sky_default_horizon() -> [f32; 3] {
    [0.58, 0.68, 0.82]
}

fn sky_default_ground() -> [f32; 3] {
    [0.16, 0.15, 0.14]
}

fn sky_default_sun_size() -> f32 {
    1.5
}

impl Default for OldSkyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            intensity: sky_default_intensity(),
            zenith: sky_default_zenith(),
            horizon: sky_default_horizon(),
            ground: sky_default_ground(),
            sun_size: sky_default_sun_size(),
            sun_intensity: sky_default_intensity(),
        }
    }
}

/// `project.ron` 投影出来的画面设置：三个宿主（CLI / 编辑器 / 游戏）都消费这一份，
/// 新增节只改这里，不再往元组末尾追加字段。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ProjectRenderSettings {
    pub(crate) sky: SkySettings,
    pub(crate) bloom: BloomSettings,
    pub(crate) fxaa: FxaaSettings,
    pub(crate) exposure: f32,
    pub(crate) shadow: ShadowSettings,
}

/// 读 `project.ron` 的画面设置（曝光 + 天空 + 光晕 + FXAA + 阴影画质）。
///
/// 没有 `project.ron` 时返回 `None`；文件在但缺节则按旧端缺省读出来
/// （曝光 1.0、其余都是"关"），与缺节等于关 / 不增不减的语义一致。
pub(crate) fn load_render(project_root: &Path) -> Option<ProjectRenderSettings> {
    let path = project_root.join("project.ron");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return None;
    };
    let project: OldProjectFile = match ron::from_str(&text) {
        Ok(project) => project,
        Err(error) => {
            // 文件在但读不懂：契约仍是"缺省即关"，但静默把天空和光晕一起关掉
            // 会让画面差异无从归因，所以留一条能查的日志。
            eprintln!(
                "warning: {} 解析失败，画面设置回落到缺省（曝光 1.0、天空 / 光晕 / FXAA / 阴影画质都关）：{error}",
                path.display()
            );
            return None;
        }
    };
    let exposure = sanitize_exposure(project.render.exposure);
    let sky = project.render.sky;
    let bloom = project.render.bloom;
    let fxaa = project.render.fxaa;
    let shadow = project.render.shadow;
    Some(ProjectRenderSettings {
        sky: SkySettings {
            enabled: sky.enabled,
            intensity: sky.intensity,
            zenith: sky.zenith,
            horizon: sky.horizon,
            ground: sky.ground,
            sun_size: sky.sun_size,
            sun_intensity: sky.sun_intensity,
        }
        .sanitized(),
        bloom: BloomSettings {
            intensity: bloom.intensity,
            threshold: bloom.threshold,
            knee: bloom.knee,
        }
        .sanitized(),
        fxaa: FxaaSettings {
            enabled: fxaa.enabled,
        },
        exposure,
        // 软阴影 / 级间混合都默认关；非有限 / 负值在消费边界落回关。
        shadow: ShadowSettings {
            softness: shadow.softness,
            cascade_blend: shadow.cascade_blend,
        }
        .sanitized(),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_sky_bloom_and_fxaa_sections_from_project_ron() {
        let root = std::path::Path::new("../../../projects/XGame");
        if !root.join("project.ron").exists() {
            eprintln!("skip: 没有旧项目 project.ron");
            return;
        }
        let settings = super::load_render(root).expect("showcase 项目应带 sky 节");
        let (sky, bloom, fxaa) = (settings.sky, settings.bloom, settings.fxaa);
        let exposure = settings.exposure;
        assert!(sky.enabled, "项目里天空是开着的");
        assert_eq!(sky.intensity, 1.0);
        assert_eq!(sky.zenith, [0.09, 0.26, 0.60]);
        assert_eq!(sky.horizon, [0.62, 0.72, 0.88]);
        assert_eq!(sky.ground, [0.15, 0.14, 0.13]);
        assert_eq!(sky.sun_size, 2.0);
        assert_eq!(sky.sun_intensity, 1.0);
        // 项目里没有 `render.bloom` 节：光晕关（画面与接入前逐位相同）。
        assert_eq!(bloom, xgc_render::BloomSettings::default());
        assert!(!bloom.is_enabled());
        // 同样没有 `render.fxaa` 节：抗锯齿关，帧输出逐字节不变。
        assert_eq!(fxaa, xgc_render::FxaaSettings::default());
        assert!(!fxaa.is_enabled());
        // 项目里显式写 `exposure: 1.0`（旧端该字段没有 skip_serializing_if，老项目
        // 总会落盘它）：1.0 走 x*1.0 的同一条乘法，帧输出逐字节不变。
        assert_eq!(exposure, xgc_render::EXPOSURE_DEFAULT);
        // 项目里没有 `render.shadow` 节：软阴影 / 级间混合关（画面逐位不变）。
        assert_eq!(settings.shadow, xgc_render::ShadowSettings::default());
        assert!(!settings.shadow.is_enabled());
        // 没有 project.ron 的资产根：天空关、光晕关。
        assert!(super::load_render(std::path::Path::new("does-not-exist")).is_none());
    }

    #[test]
    fn parses_an_explicit_exposure_and_sanitizes_illegal_ones() {
        // 显式非 1.0 必须真的被解析出来：缺省值同样是 1.0，只断言"等于 1.0"
        // 分不清"解析接线"与"兜底"。
        let explicit: super::OldProjectRender =
            ron::from_str("(exposure: 2.5)").expect("显式曝光应能解析");
        assert_eq!(explicit.exposure, 2.5);

        // 0 / 负 / 非有限在旧端都是"非法曝光"，消费边界按 sanitize_exposure 回缺省。
        let zero: super::OldProjectRender =
            ron::from_str("(exposure: 0.0)").expect("解析不该拒绝非法值");
        assert_eq!(
            xgc_render::sanitize_exposure(zero.exposure),
            xgc_render::EXPOSURE_DEFAULT
        );

        // 缺 `render` 节时的缺省必须就是 1.0（derive(Default) 会给 0.0 = 全黑帧）。
        assert_eq!(
            super::OldProjectRender::default().exposure,
            xgc_render::EXPOSURE_DEFAULT
        );
        assert_eq!(
            super::OldProjectFile::default().render.exposure,
            xgc_render::EXPOSURE_DEFAULT
        );
    }

    /// 阴影节走真实入口 `load_render`（而不是「先反序列化、再本地消毒」）：将来漏接
    /// `shadow` 字段时这条会红；「缺节 = 关」也由同一入口给出判据。
    #[test]
    fn reads_the_shadow_sections_through_the_project_entry() {
        let root =
            std::env::temp_dir().join(format!("xgc-project-render-shadow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("建临时项目目录");
        let write = |text: &str| {
            std::fs::write(root.join("project.ron"), text).expect("写临时 project.ron")
        };

        write("(render: (shadow: (softness: 0.5, cascade_blend: 0.05)))");
        let settings = super::load_render(&root).expect("临时项目应能读出画面设置");
        assert_eq!(settings.shadow.softness, 0.5);
        assert_eq!(settings.shadow.cascade_blend, 0.05);
        assert!(settings.shadow.is_enabled());
        // 没写的节保持缺省：曝光 1.0（x*1.0 与老逐字节一致），其余全关。
        assert_eq!(settings.exposure, xgc_render::EXPOSURE_DEFAULT);
        assert!(!settings.bloom.is_enabled());
        assert!(!settings.fxaa.is_enabled());

        // 负数在入口消毒成 0；`cascade_blend: 2.0` 是合法值（旧端不夹上界），保留。
        write("(render: (shadow: (softness: -0.5, cascade_blend: 2.0)))");
        let settings = super::load_render(&root).expect("垃圾值不该让解析失败");
        assert_eq!(settings.shadow.softness, 0.0);
        assert_eq!(settings.shadow.cascade_blend, 2.0);

        // 缺 `render.shadow` 节 = 关。
        write("(render: ())");
        let settings = super::load_render(&root).expect("空 render 节应走缺省");
        assert_eq!(settings.shadow, xgc_render::ShadowSettings::default());
        assert!(!settings.shadow.is_enabled());

        let _ = std::fs::remove_dir_all(&root);
    }
}
