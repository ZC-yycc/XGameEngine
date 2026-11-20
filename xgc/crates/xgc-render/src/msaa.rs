//! 场景 pass 的多重采样（MSAA）档位。
//!
//! 旧端场景 pass 默认走 4× MSAA（`crates/xgame-render/src/scene_pipeline.rs:194` 的
//! `PREFERRED_SAMPLE_COUNT`）：几何轮廓靠硬件覆盖求平均，这是旧截图里物体边缘那
//! 一两个过渡像素的来源。XGC 之前是 1×（硬边），所以第 150 帧的残余差异里有很大
//! 一块来自"边缘有没有抗锯齿"，而不是着色公式不同。
//!
//! 挑档规则与旧端一致：首选 4×，适配器不支持就退回 1×。颜色附件是 `Rgba16Float`、
//! 深度附件是 `Depth32Float`，两个格式都必须支持 4×——同一个 render pass 里的附件
//! 采样数必须一致，少一个就只能整体退回 1×（1× 等同关闭 MSAA，逐字节行为与切片前
//! 一致，所以老测试与关 MSAA 的项目不受影响）。

/// 首选采样数（与旧端 `PREFERRED_SAMPLE_COUNT` 一致）。
pub const PREFERRED_SAMPLE_COUNT: u32 = 4;

/// 这台适配器上场景 pass 该用几倍采样：`Rgba16Float` 与 `Depth32Float` 都支持 4×
/// 才用 4×，否则 1×。
pub fn preferred_sample_count(adapter: &wgpu::Adapter) -> u32 {
    let supports_x4 = |format: wgpu::TextureFormat| {
        adapter
            .get_texture_format_features(format)
            .flags
            .contains(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_X4)
    };
    if supports_x4(wgpu::TextureFormat::Rgba16Float)
        && supports_x4(wgpu::TextureFormat::Depth32Float)
    {
        PREFERRED_SAMPLE_COUNT
    } else {
        1
    }
}
