//! 光晕链的单元回归：参数容错与"链跟着源尺寸重建 / 关掉即释放"。
//!
//! 放在子模块文件里是为了守住 600 行软上限（同 `skinned/tests.rs`）：
//! 这两条需要看私有字段，不能挪到 `tests/bloom.rs` 的集成测试里。

use super::{BloomSettings, BloomStage, BLOOM_PASSES};
use crate::gpu::GpuContext;

fn source(context: &GpuContext, width: u32, height: u32) -> wgpu::Texture {
    // 链只看源纹理的尺寸与格式，内容由 GPU 侧 pass 覆盖，不必先填数据。
    context.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("xgc-bloom-test-source"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
}

#[test]
fn sanitized_matches_the_old_engine_contract() {
    let garbage = BloomSettings {
        intensity: f32::NAN,
        threshold: f32::INFINITY,
        knee: -2.0,
    }
    .sanitized();
    assert_eq!(garbage.intensity, 0.0, "NaN 强度落回缺省 0（即关闭）");
    assert_eq!(garbage.threshold, 1.0, "无穷阈值落回缺省 1.0");
    assert_eq!(garbage.knee, 0.0, "负软膝夹到 0");
    assert!(!garbage.is_enabled());

    let oversized = BloomSettings {
        intensity: 15.0,
        threshold: 1000.0,
        knee: 9.0,
    }
    .sanitized();
    assert_eq!(oversized.intensity, BloomSettings::MAX_INTENSITY);
    assert_eq!(oversized.threshold, BloomSettings::MAX_THRESHOLD);
    assert_eq!(oversized.knee, BloomSettings::MAX_KNEE);

    let negative = BloomSettings {
        intensity: -1.0,
        threshold: -3.0,
        knee: -0.5,
    }
    .sanitized();
    assert_eq!(
        (negative.intensity, negative.threshold, negative.knee),
        (0.0, 0.0, 0.0)
    );
    assert!(!negative.is_enabled(), "负强度不是几乎关闭，就是关闭");
}

#[test]
fn chain_follows_the_source_size_and_releases() {
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let mut stage = BloomStage::new(&context);
    let settings = BloomSettings {
        intensity: 1.0,
        ..BloomSettings::default()
    };

    let small = source(&context, 8, 8);
    assert_eq!(stage.render(&context, &small, settings, 1.0), BLOOM_PASSES);
    assert_eq!(stage.chain_size(), Some((8, 8)));
    assert_eq!(stage.chain_builds(), 1, "首次渲染必须建链");
    assert!(stage.views().is_some(), "跑完链必须交出三级视图");

    // 同尺寸同参数的第二帧：链必须复用（每帧重建在像素上看不出来，只能靠计数拦）。
    assert_eq!(stage.render(&context, &small, settings, 1.0), BLOOM_PASSES);
    assert_eq!(stage.chain_builds(), 1, "同尺寸同参数的第二帧不该重建链");

    // 只换曝光：亮部提取的乘子变了，链里的参数缓冲与缓存键都得跟着变。
    assert_eq!(stage.render(&context, &small, settings, 2.0), BLOOM_PASSES);
    assert_eq!(stage.chain_builds(), 2, "曝光变了链必须重建");

    // 只换源尺寸：链必须按新尺寸重建，否则模糊半径会按旧分辨率算。
    let large = source(&context, 16, 16);
    assert_eq!(stage.render(&context, &large, settings, 2.0), BLOOM_PASSES);
    assert_eq!(stage.chain_size(), Some((16, 16)), "源尺寸变了链必须重建");
    assert_eq!(stage.chain_builds(), 3);

    stage.release();
    assert_eq!(stage.chain_size(), None);
    assert!(
        stage.views().is_none(),
        "关掉光晕后不能再把残留视图绑进 tone-map"
    );
}
