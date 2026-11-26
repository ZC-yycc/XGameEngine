//! 对照旧工程截图时，复用迁移的只读配置投影。

use std::path::Path;
use xgc_base::render_settings::{RenderQuality, RenderSettings};

pub(crate) fn load_legacy_render(root: &Path) -> Option<(RenderSettings, RenderQuality)> {
    let path = root.join("project.ron");
    if matches!(path.try_exists(), Ok(false)) {
        return None;
    }
    match xgc_migrate::read_old_render(root) {
        Ok(settings) => Some(settings),
        Err(error) => {
            eprintln!(
                "warning: {} 读取画面设置失败，使用缺省值：{error}",
                path.display()
            );
            None
        }
    }
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
        let (settings, _) = super::load_legacy_render(root).expect("showcase 项目应带 sky 节");
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
        // 项目里没有 `render.taa` 节：TAA 关（相机不抖动，帧输出逐位不变）。
        assert_eq!(settings.taa, xgc_render::TaaSettings::default());
        assert!(!settings.taa.is_enabled());
        // 项目里没有 `render.ssao` 节：环境光遮蔽关（画面逐位不变）。
        assert_eq!(settings.ssao, xgc_render::SsaoSettings::default());
        assert!(!settings.ssao.is_enabled());
        // 没有 project.ron 的资产根：天空关、光晕关。
        assert!(super::load_legacy_render(std::path::Path::new("does-not-exist")).is_none());
    }

    #[test]
    fn parses_an_explicit_exposure_and_sanitizes_illegal_ones() {
        // 显式非 1.0 必须真的被解析出来：缺省值同样是 1.0，只断言"等于 1.0"
        // 分不清"解析接线"与"兜底"。
        let explicit: xgc_render::RenderSettings =
            ron::from_str("(exposure: 2.5)").expect("显式曝光应能解析");
        assert_eq!(explicit.exposure, 2.5);

        // 0 / 负 / 非有限在旧端都是"非法曝光"，消费边界按 sanitize_exposure 回缺省。
        let zero: xgc_render::RenderSettings =
            ron::from_str("(exposure: 0.0)").expect("解析不该拒绝非法值");
        assert_eq!(
            xgc_render::sanitize_exposure(zero.exposure),
            xgc_render::EXPOSURE_DEFAULT
        );

        // 缺 `render` 节时的缺省必须就是 1.0（derive(Default) 会给 0.0 = 全黑帧）。
        assert_eq!(
            xgc_render::RenderSettings::default().exposure,
            xgc_render::EXPOSURE_DEFAULT
        );
    }

    #[test]
    fn reads_lod_bias_and_quality_through_the_project_entry() {
        let root =
            std::env::temp_dir().join(format!("xgc-project-render-lod-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("建临时项目目录");
        for (ron, expected) in [
            ("()", 1.0),
            ("(render: (lod_bias: 2.0))", 2.0),
            ("(quality: (preset: Some(Low)))", 0.6),
            (
                "(render: (lod_bias: 2.0), quality: (preset: Some(High)))",
                2.5,
            ),
            (
                "(quality: (preset: Some(High), lod_scale: Some(0.75)))",
                0.75,
            ),
            (
                "(quality: (preset: Some(High), lod_scale: Some(NaN)))",
                1.25,
            ),
            ("(quality: (lod_scale: Some(-1.0)))", 0.05),
            ("(quality: (lod_scale: Some(11.0)))", 10.0),
            (
                "(render: (lod_bias: 0.0), quality: (preset: Some(Low)))",
                0.0,
            ),
        ] {
            std::fs::write(root.join("project.ron"), ron).expect("写临时 project.ron");
            let (settings, quality) =
                super::load_legacy_render(&root).expect("临时项目应能读出画面设置");
            assert_eq!(
                settings.lod_bias * quality.resolve(4).lod_scale,
                expected,
                "{ron}"
            );
        }
        std::fs::remove_dir_all(root).expect("清理临时项目目录");
    }

    /// 阴影节走真实入口 `load_legacy_render`（而不是「先反序列化、再本地消毒」）：将来漏接
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
        let (settings, _) = super::load_legacy_render(&root).expect("临时项目应能读出画面设置");
        assert_eq!(settings.shadow.softness, 0.5);
        assert_eq!(settings.shadow.cascade_blend, 0.05);
        assert!(settings.shadow.is_enabled());
        // 没写的节保持缺省：曝光 1.0（x*1.0 与老逐字节一致），其余全关。
        assert_eq!(settings.exposure, xgc_render::EXPOSURE_DEFAULT);
        assert!(!settings.bloom.is_enabled());
        assert!(!settings.taa.is_enabled());
        assert!(!settings.fxaa.is_enabled());

        // 负数在入口消毒成 0；`cascade_blend: 2.0` 是合法值（旧端不夹上界），保留。
        write("(render: (shadow: (softness: -0.5, cascade_blend: 2.0)))");
        let (settings, _) = super::load_legacy_render(&root).expect("垃圾值不该让解析失败");
        assert_eq!(settings.shadow.softness, 0.0);
        assert_eq!(settings.shadow.cascade_blend, 2.0);

        // 缺 `render.shadow` 节 = 关。
        write("(render: ())");
        let (settings, _) = super::load_legacy_render(&root).expect("空 render 节应走缺省");
        assert_eq!(settings.shadow, xgc_render::ShadowSettings::default());
        assert!(!settings.shadow.is_enabled());
        assert!(!settings.taa.is_enabled());

        // 显式开 TAA：历史权重真的被读出来并夹进 [0, 0.98]。
        write("(render: (taa: (enabled: true, history_weight: 0.95)))");
        let (settings, _) = super::load_legacy_render(&root).expect("显式 TAA 应能解析");
        assert!(settings.taa.is_enabled());
        assert_eq!(settings.taa.history_weight, 0.95);
        write("(render: (taa: (enabled: true, history_weight: 3.0)))");
        let (settings, _) = super::load_legacy_render(&root).expect("非法权重不该让解析失败");
        assert_eq!(settings.taa.history_weight, xgc_render::MAX_HISTORY_WEIGHT);

        // 显式开 SSAO：三个画质旋钮都从入口读出来，非法值在消费边界消毒。
        write("(render: (ssao: (enabled: true, strength: 2.0, radius: 3.0, bias: 0.1)))");
        let (settings, _) = super::load_legacy_render(&root).expect("显式 SSAO 应能解析");
        assert!(settings.ssao.is_enabled());
        assert_eq!(settings.ssao.strength, 2.0);
        assert_eq!(settings.ssao.radius, 3.0);
        assert_eq!(settings.ssao.bias, 0.1);
        write("(render: (ssao: (enabled: true, strength: 9.0, radius: 99.0, bias: 9.0)))");
        let (settings, _) = super::load_legacy_render(&root).expect("超上限不该让解析失败");
        assert_eq!(
            settings.ssao.strength,
            xgc_render::SsaoSettings::MAX_STRENGTH
        );
        assert_eq!(settings.ssao.radius, xgc_render::SsaoSettings::MAX_RADIUS);
        assert_eq!(settings.ssao.bias, xgc_render::SsaoSettings::MAX_BIAS);
        // 缺节 = 关（强度仍保留旧端默认值）。
        write("(render: ())");
        let (settings, _) = super::load_legacy_render(&root).expect("空 render 节应走缺省");
        assert_eq!(settings.ssao, xgc_render::SsaoSettings::default());

        let _ = std::fs::remove_dir_all(&root);
    }
}
