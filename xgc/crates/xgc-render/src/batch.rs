//! 单 render pass 批量绘制：一次 encoder / pass 提交所有实体 draw。

use super::assets::MeshCache;
use super::camera::mul;
use super::draw::DrawList;
use super::gpu::{GpuContext, GpuError};
use super::gpu_mesh::GpuMeshCache;
use super::mesh::Mesh;
use super::pipeline::RenderCache;

struct PreparedDraw {
    mesh_key: String,
    offset: u32,
}

const MATERIAL_STRIDE: u64 = 256;

pub(crate) struct BatchRequest<'a> {
    pub(crate) texture: &'a wgpu::Texture,
    pub(crate) list: &'a DrawList,
    pub(crate) meshes: &'a mut MeshCache,
    pub(crate) gpu_meshes: &'a mut GpuMeshCache,
    pub(crate) mvp: [f32; 16],
    pub(crate) depth: &'a wgpu::TextureView,
}

impl RenderCache {
    /// 所有 draw 录进同一个 render pass；颜色 / 深度只 clear 一次。
    pub(crate) fn draw_list_batched(
        &mut self,
        context: &GpuContext,
        request: &mut BatchRequest<'_>,
    ) -> Result<(), GpuError> {
        let texture = request.texture;
        let list = request.list;
        let meshes = &mut *request.meshes;
        let gpu_meshes = &mut *request.gpu_meshes;
        let mvp = request.mvp;
        let depth = request.depth;
        if list.items.is_empty() {
            return context.clear_texture(texture, wgpu::Color::BLACK);
        }
        let format = texture.format();
        let pipeline = self.depth_pipeline_for(context, format);
        let material_buffer = context.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("xgc-batch-materials"),
            size: list.items.len() as u64 * MATERIAL_STRIDE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut prepared = Vec::with_capacity(list.items.len());
        let mut slot = 0u32;
        for item in &list.items {
            let loaded = meshes
                .get(&item.model)
                .map_err(|err| GpuError::Mesh(err.to_string()))?;
            let fallback = loaded.is_none().then(Mesh::fullscreen_triangle);
            let mesh = loaded.unwrap_or_else(|| fallback.as_ref().unwrap());
            if mesh.positions.is_empty() {
                continue;
            }
            gpu_meshes.upload(context, &item.model, mesh);

            let mut uniform_bytes: Vec<u8> = item
                .color
                .iter()
                .flat_map(|component| component.to_le_bytes())
                .collect();
            for value in mul(mvp, item.transform) {
                uniform_bytes.extend_from_slice(&value.to_le_bytes());
            }
            context.queue().write_buffer(
                &material_buffer,
                u64::from(slot) * MATERIAL_STRIDE,
                &uniform_bytes,
            );
            prepared.push(PreparedDraw {
                mesh_key: item.model.clone(),
                offset: slot * MATERIAL_STRIDE as u32,
            });
            slot += 1;
        }
        let material_bind = context
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("xgc-batch-material-bind"),
                layout: pipeline.layout(),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &material_buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(80),
                    }),
                }],
            });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            context
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("xgc-batch-encoder"),
                });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xgc-batch-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(pipeline.raw());
            for draw in &prepared {
                pass.set_bind_group(0, &material_bind, &[draw.offset]);
                let Some(mesh) = gpu_meshes.get(&draw.mesh_key) else {
                    continue;
                };
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                if let Some(indices) = &mesh.index_buffer {
                    pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..mesh.indices, 0, 0..1);
                } else {
                    pass.draw(0..mesh.vertices, 0..1);
                }
            }
        }
        context.queue().submit(Some(encoder.finish()));
        Ok(())
    }
}
