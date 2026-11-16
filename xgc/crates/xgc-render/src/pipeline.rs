//! 可复用三角形管线：材质 uniform、顶点 / 索引缓冲、draw。

use super::draw::DrawList;
use super::gpu::{GpuContext, GpuError};
use super::mesh::{builtin_mesh, Mesh};

impl GpuContext {
    /// 画一个覆盖屏幕的三角形：最小可验证的管线 / 着色器 / draw call 路径。
    pub fn draw_triangle(&self, texture: &wgpu::Texture) -> Result<(), GpuError> {
        let pipeline = self.create_triangle_pipeline(texture.format());
        pipeline.draw(self, texture, [0.1, 0.8, 0.2, 1.0])
    }

    /// 创建可复用的三角形管线；颜色由 uniform 缓冲提供。
    pub fn create_triangle_pipeline(&self, format: wgpu::TextureFormat) -> TrianglePipeline {
        const TRIANGLE_WGSL: &str = r#"
struct Material {
    color: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> material: Material;

@vertex
fn vs(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    return vec4<f32>(position, 1.0);
}

@fragment
fn fs() -> @location(0) vec4<f32> {
    return material.color;
}
"#;
        let shader = self
            .device()
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("xgc-triangle"),
                source: wgpu::ShaderSource::Wgsl(TRIANGLE_WGSL.into()),
            });
        let bind_group_layout =
            self.device()
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("xgc-triangle-material"),
                    entries: &[wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    }],
                });
        let pipeline_layout =
            self.device()
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("xgc-triangle-layout"),
                    bind_group_layouts: &[&bind_group_layout],
                    push_constant_ranges: &[],
                });
        let pipeline = self
            .device()
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("xgc-triangle-pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: 12,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                    }],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview: None,
                cache: None,
            });
        TrianglePipeline {
            pipeline,
            bind_group_layout,
            format,
        }
    }
}

pub struct TrianglePipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
}

impl TrianglePipeline {
    /// 用材质颜色画覆盖屏幕的三角形；uniform 缓冲每次重建，后续按材质缓存。
    pub fn draw(
        &self,
        context: &GpuContext,
        texture: &wgpu::Texture,
        color: [f32; 4],
    ) -> Result<(), GpuError> {
        self.draw_mesh(context, texture, &Mesh::fullscreen_triangle(), color, true)
    }

    /// 在已有颜色上继续画，不 clear；用于同一次 pass 的后续 draw。
    pub fn draw_over(
        &self,
        context: &GpuContext,
        texture: &wgpu::Texture,
        color: [f32; 4],
    ) -> Result<(), GpuError> {
        self.draw_mesh(context, texture, &Mesh::fullscreen_triangle(), color, false)
    }

    /// 用真实顶点缓冲画网格；`clear` 控制是否先清黑。
    pub fn draw_mesh(
        &self,
        context: &GpuContext,
        texture: &wgpu::Texture,
        mesh: &Mesh,
        color: [f32; 4],
        clear: bool,
    ) -> Result<(), GpuError> {
        let load = if clear {
            wgpu::LoadOp::Clear(wgpu::Color::BLACK)
        } else {
            wgpu::LoadOp::Load
        };
        self.draw_with(context, texture, mesh, color, load)
    }

    fn draw_with(
        &self,
        context: &GpuContext,
        texture: &wgpu::Texture,
        mesh: &Mesh,
        color: [f32; 4],
        load: wgpu::LoadOp<wgpu::Color>,
    ) -> Result<(), GpuError> {
        if texture.format() != self.format {
            return Err(GpuError::Pipeline(format!(
                "纹理格式 {:?} 与管线 {:?} 不一致",
                texture.format(),
                self.format
            )));
        }
        let bytes: Vec<u8> = color
            .iter()
            .flat_map(|component| component.to_le_bytes())
            .collect();
        let buffer = context.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("xgc-material"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        context.queue().write_buffer(&buffer, 0, &bytes);

        if mesh.positions.is_empty() {
            return Ok(());
        }
        let vertex_bytes: Vec<u8> = mesh
            .positions
            .iter()
            .flat_map(|position| position.iter().flat_map(|value| value.to_le_bytes()))
            .collect();
        let vertex_buffer = context.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("xgc-vertices"),
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
                label: Some("xgc-indices"),
                size: index_bytes.len() as u64,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            context.queue().write_buffer(&buffer, 0, &index_bytes);
            Some(buffer)
        };
        let bind_group = context
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("xgc-material-bind"),
                layout: &self.bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            context
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("xgc-triangle-encoder"),
                });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xgc-triangle-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            if let Some(index_buffer) = &index_buffer {
                pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.indices.len() as u32, 0, 0..1);
            } else {
                pass.draw(0..mesh.positions.len() as u32, 0..1);
            }
        }
        context.queue().submit(Some(encoder.finish()));
        Ok(())
    }
}

/// 管线缓存：同一纹理格式只建一次 `TrianglePipeline`，供连续帧 / 多实体复用。
#[derive(Default)]
pub struct RenderCache {
    pipeline: Option<(wgpu::TextureFormat, TrianglePipeline)>,
}

impl RenderCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pipeline_for(
        &mut self,
        context: &GpuContext,
        format: wgpu::TextureFormat,
    ) -> &TrianglePipeline {
        let needs_rebuild = self
            .pipeline
            .as_ref()
            .is_none_or(|(current, _)| *current != format);
        if needs_rebuild {
            self.pipeline = Some((format, context.create_triangle_pipeline(format)));
        }
        &self.pipeline.as_ref().unwrap().1
    }

    pub fn draw_list(
        &mut self,
        context: &GpuContext,
        texture: &wgpu::Texture,
        list: &DrawList,
    ) -> Result<(), GpuError> {
        if list.items.is_empty() {
            return context.clear_texture(texture, wgpu::Color::BLACK);
        }
        let format = texture.format();
        for (index, item) in list.items.iter().enumerate() {
            let mesh = builtin_mesh(&item.model).unwrap_or_else(Mesh::fullscreen_triangle);
            self.pipeline_for(context, format).draw_mesh(
                context,
                texture,
                &mesh,
                item.color,
                index == 0,
            )?;
        }
        Ok(())
    }
}
