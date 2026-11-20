//! SSAO 的每帧编排（从渲染入口里抽出来，`renderer.rs` 守行数预算）。
//!
//! 关着时原样返回场景纹理，且把 AO 目标整份释放；开着时把场景色乘一遍遮蔽，
//! 得到供光晕 / tone-map 使用的 HDR 纹理，并把四趟 pass 记进统计。

use std::sync::Arc;

use super::super::camera::Mat4;
use super::super::gpu::GpuContext;

impl super::Renderer {
    /// 场景色的 SSAO 乘算。`view_projection` 必须是**未抖动**的那个矩阵。
    pub(super) fn ssao_apply(
        &mut self,
        context: &GpuContext,
        source: &Arc<wgpu::Texture>,
        view_projection: Mat4,
        camera_position: [f32; 3],
        stats: &mut super::RenderStats,
    ) -> Arc<wgpu::Texture> {
        let settings = self.ssao;
        if !settings.is_enabled() {
            if let Some(stage) = self.ssao_stage.as_mut() {
                // 关掉时整份释放 AO / scratch（留着白占显存）。
                stage.release();
            }
            return source.clone();
        }
        let sample_count = self.scene_sample_count(context);
        if let Some(stage) = self.ssao_stage.as_mut() {
            stage.set_sample_count(sample_count);
        }
        if self.ssao_stage.is_none() {
            self.ssao_stage = Some(crate::ssao::SsaoStage::new(context));
        }
        let stage = self.ssao_stage.as_mut().expect("刚确保过 SSAO 阶段");
        // 深度视图与阶段状态是 Renderer 的不同字段：分开借，避免整份再借一次。
        let depth = &self
            .depth
            .as_ref()
            .expect("SSAO 之前必须先建好深度目标")
            .view;
        let (texture, passes) = stage.render(
            context,
            source,
            depth,
            view_projection,
            camera_position,
            settings,
        );
        stats.passes += passes;
        texture
    }

    /// 当前 AO 目标尺寸（诊断 / 回归：换尺寸重建、关掉即释放）。
    pub fn ssao_target_size(&self) -> Option<(u32, u32)> {
        self.ssao_stage
            .as_ref()
            .and_then(|stage| stage.target_size())
    }
}
