//! 纹理四边形管线：把一张 RGBA8 纹理按 MVP 画到目标。

use super::gpu::{GpuContext, GpuError};
use super::mesh::Mesh;

pub struct TexturePipeline {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

const TEXTURE_WGSL: &str = r#"
struct Camera {
    mvp: mat4x4<f32>,
    tint: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;
@group(0) @binding(1)
var tex: texture_2d<f32>;
@group(0) @binding(2)
var samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@location(0) position: vec3<f32>) -> VsOut {
    var out: VsOut;
    out.pos = camera.mvp * vec4<f32>(position, 1.0);
    out.uv = vec2<f32>(position.x + 0.5, 0.5 - position.y);
    return out;
}

@fragment
fn fs(input: VsOut) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, input.uv) * camera.tint;
}
"#;

impl GpuContext {
    pub fn create_texture_pipeline(&self, format: wgpu::TextureFormat) -> TexturePipeline {
        let shader = self
            .device()
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("xgc-texture"),
                source: wgpu::ShaderSource::Wgsl(TEXTURE_WGSL.into()),
            });
        let layout = self
            .device()
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("xgc-texture-layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(80),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let pipeline_layout =
            self.device()
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("xgc-texture-pipeline-layout"),
                    bind_group_layouts: &[&layout],
                    push_constant_ranges: &[],
                });
        let pipeline = self
            .device()
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("xgc-texture-pipeline"),
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
        let sampler = self.device().create_sampler(&wgpu::SamplerDescriptor {
            label: Some("xgc-texture-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        TexturePipeline {
            pipeline,
            layout,
            sampler,
        }
    }
}

impl TexturePipeline {
    pub fn draw(
        &self,
        context: &GpuContext,
        target: &wgpu::Texture,
        texture: &wgpu::Texture,
        mvp: [f32; 16],
        tint: [f32; 4],
    ) -> Result<(), GpuError> {
        let mut bytes = Vec::with_capacity(80);
        for value in mvp {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for value in tint {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let uniform = context.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("xgc-texture-camera"),
            size: 80,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        context.queue().write_buffer(&uniform, 0, &bytes);
        let bind_group = context
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("xgc-texture-bind"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(
                            &texture.create_view(&wgpu::TextureViewDescriptor::default()),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
        let mesh = Mesh::quad();
        let positions: Vec<[f32; 3]> = if mesh.indices.is_empty() {
            mesh.positions.clone()
        } else {
            mesh.indices
                .iter()
                .filter_map(|index| mesh.positions.get(*index as usize).copied())
                .collect()
        };
        let mut vertex_bytes = Vec::with_capacity(positions.len() * 12);
        for position in &positions {
            for value in position {
                vertex_bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        let vertices = context.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("xgc-texture-vertices"),
            size: vertex_bytes.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        context.queue().write_buffer(&vertices, 0, &vertex_bytes);
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            context
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("xgc-texture-encoder"),
                });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xgc-texture-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.draw(0..positions.len() as u32, 0..1);
        }
        context.queue().submit(Some(encoder.finish()));
        Ok(())
    }
}
