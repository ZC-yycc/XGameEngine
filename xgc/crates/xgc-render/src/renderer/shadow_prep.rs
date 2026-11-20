//! 阴影的每帧准备：方向光级联拟合 + 点光立方体计划，再把 draw list 画进深度图。
//!
//! 从 `renderer.rs` 按行数预算拆出：这里只做"这一帧要不要阴影、要的话怎么投"，
//! 画面合成与显示目标留在父模块。返回值带上本帧为阴影 pass 的蒙皮竞技场新建的
//! 缓冲数、以及点光六面画了多少次，让回归能在同一台机器上直接观测"没有退化成
//! 逐投射体建缓冲 / 点光阴影没接上"。

use crate::camera::{invert, Mat4};
use crate::draw::DrawItem;
use crate::gpu::{GpuContext, GpuError};
use crate::point_shadow::{self, PointShadow};
use crate::shadow::ShadowCascades;
use xgc_world::World;

/// 一帧的阴影准备结果：方向光级联与点光立方体阴影互相独立（可以只有一种）。
pub(super) struct PreparedShadows {
    pub(super) cascades: Option<ShadowCascades>,
    pub(super) point: Option<PointShadow>,
    pub(super) skin_uniform_buffers: usize,
    pub(super) point_draw_calls: usize,
}

impl PreparedShadows {
    fn none(point: Option<PointShadow>) -> Self {
        Self {
            cascades: None,
            point,
            skin_uniform_buffers: 0,
            point_draw_calls: 0,
        }
    }
}

impl super::Renderer {
    /// 每帧按需建好深度目标 / 天空管线 / 阴影目标 / 贴图绑定。
    ///
    /// 放在这里只为行数预算：这段是"这一帧要用哪些 GPU 资源"的开关，
    /// 与阴影准备同属一个节奏，`renderer.rs` 只留一行调用。
    pub(super) fn ensure_frame_targets(
        &mut self,
        context: &GpuContext,
        target: &wgpu::Texture,
    ) -> Result<(), GpuError> {
        self.ensure_depth(context, target.width(), target.height())?;
        // 天空管线要在拿 `self.depth` 的不可变借用之前建好（之后只剩读）。
        if self.sky.enabled {
            let format = target.format();
            let sample_count = self.scene_sample_count(context);
            if self
                .sky_pipeline
                .as_ref()
                .is_none_or(|(current, count, _)| *current != format || *count != sample_count)
            {
                self.sky_pipeline = Some((
                    format,
                    sample_count,
                    context.create_sky_pipeline(format, sample_count),
                ));
            }
        }
        if self.shadow.is_none() {
            self.shadow = Some(crate::shadow::ShadowTarget::new(context));
        }
        if self.textures.is_none() {
            self.textures = Some(crate::texture_bind::TextureBinding::new(context));
        }
        Ok(())
    }

    /// 没有可用方向光 / 点光 / 场景没有包围盒 / 相机矩阵不可逆时，对应那一种阴影为
    /// `None`（等于不投影）；两种都没有时整段跳过。
    pub(super) fn prepare_shadow(
        &mut self,
        context: &GpuContext,
        world: &World,
        items: &[DrawItem],
        view_proj: Mat4,
        camera_pos: [f32; 3],
        cascade_blend: f32,
    ) -> Result<PreparedShadows, GpuError> {
        let environment = world.environment();
        // 点光阴影与相机无关：只认第一盏 casts_shadow 且 range > 0 的点光（旧端同判据）。
        let point = point_shadow::pick_light(environment).map(point_shadow::plan);
        let mut prepared = PreparedShadows::none(point);
        // 主阴影光与 `lights::pack` 用同一判据：只有它会在着色里带 `shadow_params.x = 1`。
        // 分别扫环境表会漏掉"主阴影光排在第 9 盏之后"的情形，白跑一次级联深度 pass。
        if let Some(light) = crate::lights::primary_shadow_light(environment) {
            if let Some(bounds) = crate::shot::scene_bounds(world, &mut self.meshes)
                .map_err(|err| GpuError::Mesh(err.to_string()))?
            {
                if let Some(inverse) = invert(view_proj) {
                    prepared.cascades = crate::shadow::cascade_shadow_matrices(
                        inverse,
                        camera_pos,
                        light.normalized_direction(),
                        &bounds,
                        crate::shadow::SHADOW_RESOLUTION,
                        crate::shadow::SHADOW_CASCADES,
                        // 级间混合开着时每级（非第 0 级）近端要多铺一段，切分前的混合带
                        // 才在两级里都采得到；0 = 老拟合，逐位一致。
                        cascade_blend,
                    );
                }
            }
        }
        if prepared.cascades.is_none() && prepared.point.is_none() {
            return Ok(prepared);
        }
        if self.textures.is_none() {
            self.textures = Some(crate::texture_bind::TextureBinding::new(context));
        }
        // 蒙皮投射体的 palette / model 并进阴影自己的竞技场，所以这里要可变的 target。
        let Some(target) = self.shadow.as_mut() else {
            return Ok(prepared);
        };
        let mut sources = crate::shadow_skin::CasterSources {
            meshes: &mut self.meshes,
            gpu_meshes: &mut self.gpu_meshes,
            skin_meshes: &mut self.skin_meshes,
            textures: self.textures.as_ref().unwrap(),
        };
        let stats = target
            .render(
                context,
                prepared.cascades.as_ref(),
                prepared.point.as_ref(),
                items,
                &mut sources,
            )
            .map_err(GpuError::Pipeline)?;
        prepared.skin_uniform_buffers = stats.skin_uniform_buffers;
        prepared.point_draw_calls = stats.point_draw_calls;
        Ok(prepared)
    }
}
