//! 帧渲染入口：DrawList → MeshCache → RenderCache → GPU。

use xgc_world::World;

use super::assets::MeshCache;
use super::camera::{camera_view_projection, IDENTITY};
use super::draw::build_draw_list;
use super::gpu::{GpuContext, GpuError};
use super::pipeline::RenderCache;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    pub draws: usize,
}

pub struct Renderer {
    cache: RenderCache,
    meshes: MeshCache,
}

impl Renderer {
    pub fn new(meshes: MeshCache) -> Self {
        Self {
            cache: RenderCache::new(),
            meshes,
        }
    }

    pub fn meshes(&self) -> &MeshCache {
        &self.meshes
    }

    /// 渲染一帧：抽 DrawList、解析模型、按实体顺序 draw 到目标纹理。
    pub fn render(
        &mut self,
        context: &GpuContext,
        target: &wgpu::Texture,
        world: &World,
    ) -> Result<RenderStats, GpuError> {
        let list = build_draw_list(world);
        let mvp =
            camera_view_projection(world, target.width(), target.height()).unwrap_or(IDENTITY);
        self.cache.draw_list_with_meshes_and_camera(
            context,
            target,
            &list,
            &mut self.meshes,
            mvp,
        )?;
        Ok(RenderStats { draws: list.len() })
    }
}
