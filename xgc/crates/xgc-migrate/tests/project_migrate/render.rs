use super::*;
use xgc_base::render_settings::ToneMapMode;

#[test]
fn migrates_project_render_configuration() {
    let (root, out) = standard_project("render");
    let path = root.join("project.ron");
    let text = fs::read_to_string(&path).unwrap();
    let text = text.trim_end().strip_suffix(')').unwrap();
    fs::write(
        &path,
        format!(
            r#"{text}
                render: (
                    exposure: 2.25, tone_map: Off, lod_bias: 1.5,
                    sky: (enabled: true, horizon: (0.2, 0.4, 0.6)),
                    bloom: (intensity: 0.75), fxaa: (enabled: true),
                    taa: (enabled: true), ssao: (enabled: true, strength: 1.4),
                    shadow: (softness: 0.2),
                ),
                quality: (preset: Some(Low), lod_scale: Some(0.8)),
            )"#
        ),
    )
    .unwrap();
    migrate_old_project(&root, &out, false).unwrap();
    let manifest = load_project(&out.join("project.xgc")).unwrap();
    let (render, quality) = xgc_migrate::read_old_render(&root).unwrap();
    assert_eq!((manifest.render, manifest.quality), (render, quality));
    assert_eq!(render.exposure, 2.25);
    assert_eq!(render.tone_map, ToneMapMode::Off);
    assert_eq!(render.sky.horizon, [0.2, 0.4, 0.6]);
    assert!(render.sky.enabled && render.bloom.is_enabled() && render.fxaa.is_enabled());
    assert!(render.taa.is_enabled() && render.ssao.is_enabled() && render.shadow.is_enabled());
    let resolved = quality.resolve(4);
    assert_eq!(resolved.msaa, 1);
    assert_eq!(resolved.shadow_resolution, 1024);
    assert_eq!(render.lod_bias * resolved.lod_scale, 1.5 * 0.8);
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&out);
}
