//! 天空 pass 的每帧绘制（从 `renderer.rs` 按行数预算拆出）。
//!
//! 单独一个 pass（`LoadOp::Load`）而不是塞进实例化 pass：实例化 pass 自己
//! 负责清屏与深度清除，天空必须排在所有不透明几何之后。

use super::super::camera::{invert, Mat4, IDENTITY};
use super::super::frame_uniform::FrameLighting;
use super::super::gpu::GpuContext;

impl super::Renderer {
    /// 天空盒：整屏一张三角形，深度 `LessEqual` + 不写深度，被几何挡住的像素不画。
    pub(super) fn draw_sky(
        &self,
        context: &GpuContext,
        view: &wgpu::TextureView,
        target: super::SceneTarget,
        view_proj: Mat4,
        lighting: &FrameLighting,
        clear_depth: bool,
    ) {
        let _ = target;
        let Some((_, _, sky_pipeline)) = self.sky_pipeline.as_ref() else {
            return;
        };
        let depth = &self.depth.as_ref().unwrap().view;
        let sun = (lighting.sun_intensity > 0.0).then(|| {
            let [x, y, z] = lighting.sun_direction;
            ([-x, -y, -z], lighting.sun_color)
        });
        let uniform = crate::sky::SkyUniform::build(&self.sky, sun);
        let mut encoder =
            context
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("xgc-sky-encoder"),
                });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xgc-sky-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth,
                    depth_ops: Some(wgpu::Operations {
                        load: if clear_depth {
                            wgpu::LoadOp::Clear(1.0)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            sky_pipeline.draw(
                context,
                &mut pass,
                invert(view_proj).unwrap_or(IDENTITY),
                lighting.eye,
                &uniform,
            );
        }
        context.queue().submit(Some(encoder.finish()));
    }
}
