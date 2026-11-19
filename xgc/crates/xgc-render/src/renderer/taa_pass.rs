//! TAA 的每帧编排：抖动相机、解析历史、把结果交给 tone-map。
//!
//! 两个渲染入口（`render_selected` / `render_shot`）共用这两步：
//!
//! 1. [`Renderer::taa_begin`]：开 TAA 时建阶段、推进抖动相位，返回**抖动后**的
//!    场景视图投影；关着时释放历史并原样返回。
//! 2. [`Renderer::taa_resolve`]：场景画完后把颜色 + 深度解析成"可采样的一帧"；
//!    关着时原样返回场景纹理。返回的纹理给光晕链与 tone-map 用。
//!
//! 阴影拟合拿到的仍是未抖动矩阵（调用方分别传两个矩阵），满足旧端"抖动只叠相机"
//! 的纪律。

use std::sync::Arc;

use crate::camera::Mat4;
use crate::gpu::GpuContext;
use crate::taa::{jitter_projection, jitter_to_ndc, ResolveInput, TaaSettings, TaaStage};

/// 一帧的两套视图投影：场景用抖动后的（TAA），阴影拟合必须用未抖动的。
#[derive(Clone, Copy)]
pub(super) struct CameraMatrices {
    pub(super) scene: Mat4,
    pub(super) shadow: Mat4,
}

impl super::Renderer {
    /// 开帧：返回场景要用的视图投影（打开时已叠抖动）。
    pub(super) fn taa_begin(
        &mut self,
        context: &GpuContext,
        width: u32,
        height: u32,
        view_proj: Mat4,
    ) -> Mat4 {
        let settings = self.taa;
        if !settings.is_enabled() {
            if let Some(stage) = self.taa_stage.as_mut() {
                stage.release();
            }
            return view_proj;
        }
        let stage = self.taa_stage.get_or_insert_with(|| TaaStage::new(context));
        let jitter = stage.begin_frame((width, height));
        jitter_projection(view_proj, jitter_to_ndc(jitter, (width, height)))
    }

    /// 收帧：解析出可采样的颜色纹理（关着时就是传入的场景纹理）。
    pub(super) fn taa_resolve(
        &mut self,
        context: &GpuContext,
        scene: &Arc<wgpu::Texture>,
        view_proj: Mat4,
        camera_pos: [f32; 3],
        stats: &mut super::RenderStats,
    ) -> Arc<wgpu::Texture> {
        let settings: TaaSettings = self.taa;
        if !settings.is_enabled() {
            return Arc::clone(scene);
        }
        let size = (scene.width(), scene.height());
        let scene_view = scene.create_view(&wgpu::TextureViewDescriptor::default());
        let depth = &self
            .depth
            .as_ref()
            .expect("TAA 解析前必须先建好深度目标")
            .view;
        let stage = self.taa_stage.as_mut().expect("taa_begin 已建好阶段");
        let (resolved, passes) = stage.resolve(
            context,
            ResolveInput {
                color: &scene_view,
                depth,
                size,
            },
            view_proj,
            camera_pos,
            settings,
        );
        stats.passes += passes;
        resolved
    }
}

impl super::Renderer {
    /// 相机瞬移 / 切场景后调用：保留历史纹理、清重投影状态，下一帧直通当前帧。
    /// 现在还没有调用方（编辑器视口接相机跳转时用），但合同先钉住。
    pub fn reset_taa_history(&mut self) {
        if let Some(stage) = self.taa_stage.as_mut() {
            stage.reset_history();
        }
    }

    /// 重投影状态是否刚被重置（诊断 / 回归用；关着 TAA 时恒为真）。
    pub fn taa_reprojection_is_reset(&self) -> bool {
        match self.taa_stage.as_ref() {
            Some(stage) => stage.reprojection_is_reset(),
            None => true,
        }
    }

    /// 当前 TAA 历史纹理尺寸（`None` = 没建历史）；诊断与回归用来确认"关着不建、
    /// 换尺寸重建、关掉即释放"。
    pub fn taa_history_size(&self) -> Option<(u32, u32)> {
        self.taa_stage
            .as_ref()
            .and_then(|stage| stage.history_size())
    }
}

#[cfg(test)]
mod tests {
    use crate::assets::MeshCache;
    use crate::camera::IDENTITY;
    use crate::gpu::GpuContext;
    use crate::taa::TaaSettings;
    use crate::Renderer;

    /// 接线判据：开着时返回**抖动后**的投影，且每帧相位不同；关着时原样返回。
    /// （像素级只能看"历史有没有混"，抖动是否真的接进场景矩阵要靠这条钉住。）
    #[test]
    fn begin_returns_a_jittered_projection_only_when_enabled() {
        let Ok(context) = GpuContext::headless() else {
            eprintln!("skip: 没有可用 GPU 适配器");
            return;
        };
        let mut renderer = Renderer::new(MeshCache::new("."));
        assert!(
            renderer.taa_begin(&context, 64, 64, IDENTITY) == IDENTITY,
            "关着 TAA 时投影必须原样返回"
        );
        renderer.set_taa(TaaSettings {
            enabled: true,
            ..TaaSettings::default()
        });
        let first = renderer.taa_begin(&context, 64, 64, IDENTITY);
        let second = renderer.taa_begin(&context, 64, 64, IDENTITY);
        assert!(first != IDENTITY, "开着 TAA 时投影必须被抖动");
        assert!(first != second, "抖动相位必须逐帧推进");
    }
}
