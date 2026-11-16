//! 单 render pass 批量绘制：一次 encoder / pass 提交所有实体 draw。

use super::assets::MeshCache;
use super::camera::mul;
use super::draw::DrawList;
use super::gpu::{GpuContext, GpuError};
use super::mesh::Mesh;
use super::pipeline::RenderCache;

struct PreparedDraw {
    bind_group: wgpu::BindGroup,
    vertex_buffer: wgpu::Buffer,
    index_buffer: Option<wgpu::Buffer>,
    vertices: u32,
    indices: u32,
}

impl RenderCache {
    /// 所有 draw 录进同一个 render pass；颜色 / 深度只 clear 一次。
    pub fn draw_list_batched(
        &mut self,
        context: &GpuContext,
        texture: &wgpu::Texture,
        list: &DrawList,
        meshes: &mut MeshCache,
        mvp: [f32; 16],
        depth: &wgpu::TextureView,
    ) -> Result<(), GpuError> {
        if list.items.is_empty() {
            return context.clear_texture(texture, wgpu::Color::BLACK);
        }
        let format = texture.format();
        let pipeline = self.depth_pipeline_for(context, format);
        let mut prepared = Vec::with_capacity(list.items.len());
        for item in &list.items {
            let loaded = meshes
                .get(&item.model)
                .map_err(|err| GpuError::Mesh(err.to_string()))?;
            let fallback = loaded.is_none().then(Mesh::fullscreen_triangle);
            let mesh = loaded.unwrap_or_else(|| fallback.as_ref().unwrap());
            if mesh.positions.is_empty() {
                continue;
            }
            let vertex_bytes: Vec<u8> = mesh
                .positions
                .iter()
                .flat_map(|position| position.iter().flat_map(|value| value.to_le_bytes()))
                .collect();
            let vertex_buffer = context.device().create_buffer(&wgpu::BufferDescriptor {
                label: Some("xgc-batch-vertices"),
                size: vertex_bytes.len() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            context
                .queue()
                .write_buffer(&vertex_buffer, 0, &vertex_bytes);
            let index_buffer = if mesh.indices.is_empty() {
                None
            } else {
                let index_bytes: Vec<u8> = mesh
                    .indices
                    .iter()
                    .flat_map(|index| index.to_le_bytes())
                    .collect();
                let buffer = context.device().create_buffer(&wgpu::BufferDescriptor {
                    label: Some("xgc-batch-indices"),
                    size: index_bytes.len() as u64,
                    usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                context.queue().write_buffer(&buffer, 0, &index_bytes);
                Some(buffer)
            };

            let mut uniform_bytes: Vec<u8> = item
                .color
                .iter()
                .flat_map(|component| component.to_le_bytes())
                .collect();
            for value in mul(mvp, item.transform) {
                uniform_bytes.extend_from_slice(&value.to_le_bytes());
            }
            let uniform = context.device().create_buffer(&wgpu::BufferDescriptor {
                label: Some("xgc-batch-material"),
                size: 80,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            context.queue().write_buffer(&uniform, 0, &uniform_bytes);
            let bind_group = context
                .device()
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("xgc-batch-material-bind"),
                    layout: pipeline.layout(),
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    }],
                });
            prepared.push(PreparedDraw {
                bind_group,
                vertex_buffer,
                index_buffer,
                vertices: mesh.positions.len() as u32,
                indices: mesh.indices.len() as u32,
            });
        }

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
                pass.set_bind_group(0, &draw.bind_group, &[]);
                pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                if let Some(indices) = &draw.index_buffer {
                    pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..draw.indices, 0, 0..1);
                } else {
                    pass.draw(0..draw.vertices, 0..1);
                }
            }
        }
        context.queue().submit(Some(encoder.finish()));
        Ok(())
    }
}
