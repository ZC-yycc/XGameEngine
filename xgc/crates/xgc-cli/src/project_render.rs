//! `project.ron` 的画面设置读取：天空与光晕。
//!
//! 旧引擎把画面设置放在项目级而不是场景级，XGC 也按项目级读；缺省值必须逐项
//! 等于旧端 `SkyConfig::default` / `BloomConfig::default`，缺文件 / 缺节时
//! 画面才与接入前逐位相同。

use std::path::Path;

use serde::Deserialize;
use xgc_render::{BloomSettings, SkySettings};

/// `project.ron` 的 `render` 节里 XGC 目前真正消费的部分（天空 + 光晕）。
#[derive(Debug, Default, Deserialize)]
struct OldProjectFile {
    #[serde(default)]
    render: OldProjectRender,
}

#[derive(Debug, Default, Deserialize)]
struct OldProjectRender {
    #[serde(default)]
    sky: OldSkyConfig,
    #[serde(default)]
    bloom: OldBloomConfig,
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

/// 读 `project.ron` 的画面设置（天空 + 光晕）。
///
/// 没有 `project.ron` 时返回 `None`；文件在但缺 `sky` / `bloom` 节则按缺省值
/// （都是"关"）读出来，与缺节等于关的语义一致。
pub(crate) fn load_render(project_root: &Path) -> Option<(SkySettings, BloomSettings)> {
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
                "warning: {} 解析失败，画面设置回落到缺省（天空与光晕都关）：{error}",
                path.display()
            );
            return None;
        }
    };
    let sky = project.render.sky;
    let bloom = project.render.bloom;
    Some((
        SkySettings {
            enabled: sky.enabled,
            intensity: sky.intensity,
            zenith: sky.zenith,
            horizon: sky.horizon,
            ground: sky.ground,
            sun_size: sky.sun_size,
            sun_intensity: sky.sun_intensity,
        }
        .sanitized(),
        BloomSettings {
            intensity: bloom.intensity,
            threshold: bloom.threshold,
            knee: bloom.knee,
        }
        .sanitized(),
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_sky_and_bloom_sections_from_project_ron() {
        let root = std::path::Path::new("../../../projects/XGame");
        if !root.join("project.ron").exists() {
            eprintln!("skip: 没有旧项目 project.ron");
            return;
        }
        let (sky, bloom) = super::load_render(root).expect("showcase 项目应带 sky 节");
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
        // 没有 project.ron 的资产根：天空关、光晕关。
        assert!(super::load_render(std::path::Path::new("does-not-exist")).is_none());
    }
}
