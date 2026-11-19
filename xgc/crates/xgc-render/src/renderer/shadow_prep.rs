//! 方向光阴影的每帧准备：拟合级联、把 draw list 画进深度数组。
//!
//! 从 `renderer.rs` 按行数预算拆出：这里只做"这一帧要不要阴影、要的话怎么投"，
//! 画面合成与显示目标留在父模块。返回值里带上本帧为阴影 pass 的蒙皮竞技场新建的
//! 缓冲数，好让回归能在同一台机器上直接观测"没有退化成逐投射体建缓冲"。

use crate::camera::{invert, Mat4};
use crate::draw::DrawItem;
use crate::gpu::{GpuContext, GpuError};
use crate::shadow::ShadowCascades;
use xgc_world::World;

impl super::Renderer {
    /// 没有可用方向光 / 场景没有包围盒 / 相机矩阵不可逆时返回 `None`（等于不投影）。
    pub(super) fn prepare_shadow(
        &mut self,
        context: &GpuContext,
        world: &World,
        items: &[DrawItem],
        view_proj: Mat4,
        camera_pos: [f32; 3],
        cascade_blend: f32,
    ) -> Result<(Option<ShadowCascades>, usize), GpuError> {
        let Some(light) = world.environment().lights.iter().find(|light| {
            light.kind == xgc_world::LightKind::Directional
                && light.is_active()
                && light.casts_shadow
        }) else {
            return Ok((None, 0));
        };
        let bounds = crate::shot::scene_bounds(world, &mut self.meshes)
            .map_err(|err| GpuError::Mesh(err.to_string()))?;
        let Some(bounds) = bounds else {
            return Ok((None, 0));
        };
        let Some(inverse) = invert(view_proj) else {
            return Ok((None, 0));
        };
        let Some(cascades) = crate::shadow::cascade_shadow_matrices(
            inverse,
            camera_pos,
            light.normalized_direction(),
            &bounds,
            crate::shadow::SHADOW_RESOLUTION,
            crate::shadow::SHADOW_CASCADES,
            // 级间混合开着时每级（非第 0 级）近端要多铺一段，切分前的混合带
            // 才在两级里都采得到；0 = 老拟合，逐位一致。
            cascade_blend,
        ) else {
            return Ok((None, 0));
        };
        if self.textures.is_none() {
            self.textures = Some(crate::texture_bind::TextureBinding::new(context));
        }
        // 蒙皮投射体的 palette / model 并进阴影自己的竞技场，所以这里要可变的 target。
        let Some(target) = self.shadow.as_mut() else {
            return Ok((Some(cascades), 0));
        };
        let mut sources = crate::shadow_skin::CasterSources {
            meshes: &mut self.meshes,
            gpu_meshes: &mut self.gpu_meshes,
            skin_meshes: &mut self.skin_meshes,
            textures: self.textures.as_ref().unwrap(),
        };
        let skin_uniform_buffers = target
            .render(context, &cascades, items, &mut sources)
            .map_err(GpuError::Pipeline)?;
        Ok((Some(cascades), skin_uniform_buffers))
    }
}
