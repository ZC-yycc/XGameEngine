//! HDR 中间目标回归：resize、输出格式管线缓存、线性清屏与最终 alpha。

use xgc_render::{GpuContext, GpuResource, MeshCache, Renderer, ResourceDesc, ResourceUsage};
use xgc_world::World;

#[test]
fn hdr_target_resizes_and_tone_pipeline_tracks_output_format() {
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let mut renderer = Renderer::new(MeshCache::new("."));
    let world = World::new();
    let cases = [
        (4, 4, "rgba8unorm", [2.0, 0.18, 0.0]),
        (7, 3, "rgba8unorm-srgb", [0.0, 0.18, 2.0]),
        (7, 3, "rgba8unorm-srgb", [0.18, 2.0, 0.0]),
        (2, 5, "rgba8unorm", [0.0, 2.0, 0.18]),
    ];
    for (index, (width, height, format, linear)) in cases.into_iter().enumerate() {
        let desc = ResourceDesc::texture2d(
            format!("hdr-resize-{index}"),
            format,
            width,
            height,
            vec![ResourceUsage::ColorAttachment],
        );
        let GpuResource::Texture(texture) = context.create_resource(&desc).unwrap() else {
            panic!("HDR resize target 不是纹理");
        };
        renderer.set_clear_color([
            f64::from(linear[0]),
            f64::from(linear[1]),
            f64::from(linear[2]),
            0.0,
        ]);
        let stats = renderer.render(&context, &texture, &world).unwrap();
        assert_eq!(stats.passes, 2, "HDR 清屏与 tone-map 各是一个阶段");
        let pixels = context.read_texture_rgba8(&texture, width, height).unwrap();
        let expected = if format.ends_with("-srgb") {
            xgc_render::display_u8(linear)
        } else {
            xgc_render::aces_u8(linear)
        };
        for (axis, want) in expected.into_iter().enumerate() {
            let got = pixels[axis];
            assert!(
                (i16::from(got) - i16::from(want)).abs() <= 2,
                "第 {index} 次 resize/format 切换通道 {axis}：实际 {got}，期望 {want}"
            );
        }
        assert_eq!(pixels[3], 255, "tone-map 最终目标必须不透明");
    }
}
