//! 帧渲染入口：DrawList → MeshCache → RenderCache → GPU。

use xgc_world::World;

use super::assets::MeshCache;
use super::batch::BatchRequest;
use super::camera::{camera_view_projection, IDENTITY};
use super::draw::build_draw_list;
use super::gpu::{GpuContext, GpuError};
use super::gpu_material::GpuMaterialBatch;
use super::gpu_mesh::GpuMeshCache;
use super::pipeline::RenderCache;
use super::resources::{ResourceDesc, ResourceUsage};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    pub draws: usize,
    pub passes: usize,
}

struct DepthTarget {
    width: u32,
    height: u32,
    view: wgpu::TextureView,
}

pub struct Renderer {
    cache: RenderCache,
    meshes: MeshCache,
    gpu_meshes: GpuMeshCache,
    materials: GpuMaterialBatch,
    depth: Option<DepthTarget>,
}

impl Renderer {
    pub fn new(meshes: MeshCache) -> Self {
        Self {
            cache: RenderCache::new(),
            meshes,
            gpu_meshes: GpuMeshCache::new(),
            materials: GpuMaterialBatch::new(),
            depth: None,
        }
    }

    pub fn meshes(&self) -> &MeshCache {
        &self.meshes
    }

    pub fn gpu_mesh_count(&self) -> usize {
        self.gpu_meshes.len()
    }

    pub fn material_capacity(&self) -> u64 {
        self.materials.capacity()
    }

    fn ensure_depth(
        &mut self,
        context: &GpuContext,
        width: u32,
        height: u32,
    ) -> Result<(), GpuError> {
        if self
            .depth
            .as_ref()
            .is_some_and(|depth| depth.width == width && depth.height == height)
        {
            return Ok(());
        }
        let desc = ResourceDesc::texture2d(
            "xgc-depth",
            "depth32float",
            width.max(1),
            height.max(1),
            vec![ResourceUsage::DepthAttachment],
        );
        let resource = context.create_resource(&desc)?;
        let texture = resource
            .into_texture()
            .ok_or_else(|| GpuError::Pipeline("depth 资源不是纹理".to_string()))?;
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.depth = Some(DepthTarget {
            width,
            height,
            view,
        });
        Ok(())
    }

    /// 渲染一帧：抽 DrawList、解析模型、按实体顺序 draw 到目标纹理。
    pub fn render(
        &mut self,
        context: &GpuContext,
        target: &wgpu::Texture,
        world: &World,
    ) -> Result<RenderStats, GpuError> {
        let width = target.width();
        let height = target.height();
        self.ensure_depth(context, width, height)?;
        let list = build_draw_list(world);
        let mvp = camera_view_projection(world, width, height).unwrap_or(IDENTITY);
        let depth = &self.depth.as_ref().unwrap().view;
        let mut request = BatchRequest {
            texture: target,
            list: &list,
            meshes: &mut self.meshes,
            gpu_meshes: &mut self.gpu_meshes,
            materials: &mut self.materials,
            mvp,
            depth,
        };
        self.cache.draw_list_batched(context, &mut request)?;
        Ok(RenderStats {
            draws: list.len(),
            passes: usize::from(!list.is_empty()),
        })
    }
}
