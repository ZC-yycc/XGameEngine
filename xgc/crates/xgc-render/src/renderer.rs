//! 帧渲染入口：DrawList → MeshCache → RenderCache → GPU。

use xgc_world::{UiElement, UiInteraction, UiLayout, World};

use super::assets::MeshCache;
use super::camera::{camera_view_projection, IDENTITY};
use super::draw::build_draw_list;
use super::gpu::{GpuContext, GpuError};
use super::gpu_mesh::GpuMeshCache;
use super::instanced::InstancedRequest;
use super::mesh::Mesh;
use super::pipeline::RenderCache;
use super::resources::{ResourceDesc, ResourceUsage};
use super::ui_draw::quad_matrix;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    pub draws: usize,
    pub draw_calls: usize,
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
    depth: Option<DepthTarget>,
}

impl Renderer {
    pub fn new(meshes: MeshCache) -> Self {
        Self {
            cache: RenderCache::new(),
            meshes,
            gpu_meshes: GpuMeshCache::new(),
            depth: None,
        }
    }

    pub fn meshes(&self) -> &MeshCache {
        &self.meshes
    }

    pub fn gpu_mesh_count(&self) -> usize {
        self.gpu_meshes.len()
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

    /// 渲染 UI 背景矩形：在已渲染画面上叠加，不写深度；返回 draw 数。
    pub fn render_ui(
        &mut self,
        context: &GpuContext,
        target: &wgpu::Texture,
        world: &World,
    ) -> Result<usize, GpuError> {
        let viewport = [target.width() as f32, target.height() as f32];
        let pipeline = context.create_triangle_pipeline(target.format());
        let mesh = Mesh::quad();
        let mut items = Vec::new();
        world.for_each::<UiLayout>(|key, layout| {
            if !layout.visible {
                return;
            }
            let color = world
                .get::<UiElement>(key)
                .map(|element| element.background)
                .unwrap_or([1.0; 4]);
            if color[3] <= 0.0 {
                return;
            }
            let focused = world
                .get::<UiInteraction>(key)
                .is_some_and(|state| state.focused);
            if let Some(entity) = world.entity(key) {
                items.push((entity, layout.rect, color, focused));
            }
        });
        items.sort_by_key(|(entity, _, _, _)| *entity);
        let mut draws = 0;
        for (_, rect, color, focused) in &items {
            if *focused {
                let ring = [rect[0] - 2.0, rect[1] - 2.0, rect[2] + 4.0, rect[3] + 4.0];
                pipeline.draw_mesh_with_mvp(
                    context,
                    target,
                    &mesh,
                    [1.0, 0.8, 0.2, 1.0],
                    false,
                    Some(quad_matrix(ring, viewport)),
                )?;
                draws += 1;
            }
            pipeline.draw_mesh_with_mvp(
                context,
                target,
                &mesh,
                *color,
                false,
                Some(quad_matrix(*rect, viewport)),
            )?;
            draws += 1;
        }
        Ok(draws)
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
        let view_proj = camera_view_projection(world, width, height).unwrap_or(IDENTITY);
        let depth = &self.depth.as_ref().unwrap().view;
        let mut request = InstancedRequest {
            texture: target,
            list: &list,
            meshes: &mut self.meshes,
            gpu_meshes: &mut self.gpu_meshes,
            view_proj,
            depth,
        };
        let draw_calls = self.cache.draw_list_instanced(context, &mut request)?;
        Ok(RenderStats {
            draws: list.len(),
            draw_calls,
            passes: usize::from(!list.is_empty()),
        })
    }
}
