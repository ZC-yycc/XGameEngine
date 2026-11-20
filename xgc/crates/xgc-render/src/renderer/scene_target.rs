//! 场景目标的采样档位：深度附件与采样数口径都从这里取。
//!
//! 单拆一个文件只为 `renderer.rs` 的行数预算：MSAA 让"目标"从一张纹理变成
//! 颜色附件 + resolve 目标 + 深度附件三件套，采样数必须四处一致（附件、
//! 管线、深度绑定、TAA / SSAO 的着色器声明），集中在这里免得各处漂移。

use super::super::gpu::{GpuContext, GpuError};
use super::Renderer;

/// 场景目标是哪一档（输出格式 + 采样数）：管线缓存与清屏路径都按这对键分支。
#[derive(Clone, Copy)]
pub(super) struct SceneTarget {
    pub(super) format: wgpu::TextureFormat,
    pub(super) sample_count: u32,
}

pub(super) struct DepthTarget {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) sample_count: u32,
    pub(super) view: wgpu::TextureView,
}

impl Renderer {
    pub(super) fn scene_sample_count(&self, context: &GpuContext) -> u32 {
        self.sample_count
            .unwrap_or_else(|| context.scene_sample_count())
    }

    pub(super) fn ensure_depth(
        &mut self,
        context: &GpuContext,
        width: u32,
        height: u32,
    ) -> Result<(), GpuError> {
        let sample_count = self.scene_sample_count(context);
        if self.depth.as_ref().is_some_and(|depth| {
            depth.width == width && depth.height == height && depth.sample_count == sample_count
        }) {
            return Ok(());
        }
        // 深度附件必须与颜色附件采样数一致：场景开 MSAA 时这里也得是多采样纹理；
        // `TEXTURE_BINDING` 是给 TAA / SSAO 读深度用的（历史重投影要世界坐标）。
        let texture = context.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("xgc-depth"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.depth = Some(DepthTarget {
            width,
            height,
            sample_count,
            view,
        });
        Ok(())
    }
}
