//! 字体绘制管线：一次 draw 把一批字形四边形画到目标上（alpha 混合、不写深度）。
//!
//! 输入是 `font::FontAtlas` 的 RGBA8 图集（rgb=255、alpha=覆盖率）与
//! `font::GlyphQuad` 列表；顶点里带像素坐标 + uv + 颜色，片元采样后乘颜色。

use super::font::GlyphQuad;
use super::gpu::{GpuContext, GpuError};

const TEXT_WGSL: &str = r#"
struct Screen {
    size: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> screen: Screen;
@group(0) @binding(1)
var atlas_texture: texture_2d<f32>;
@group(0) @binding(2)
var atlas_sampler: sampler;

struct VsIn {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs(input: VsIn) -> VsOut {
    var out: VsOut;
    let ndc = vec2<f32>(
        input.position.x / screen.size.x * 2.0 - 1.0,
        1.0 - input.position.y / screen.size.y * 2.0,
    );
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = input.uv;
    out.color = input.color;
    return out;
}

@fragment
fn fs(input: VsOut) -> @location(0) vec4<f32> {
    let coverage = textureSample(atlas_texture, atlas_sampler, input.uv).a;
    return vec4<f32>(input.color.rgb, input.color.a * coverage);
}
"#;

pub struct TextPipeline {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

/// 顶点：位置 8B + uv 8B + 颜色 16B = 32B。
pub const TEXT_VERTEX_BYTES: usize = 32;

impl GpuContext {
    pub fn create_text_pipeline(&self, format: wgpu::TextureFormat) -> TextPipeline {
        let shader = self
            .device()
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("xgc-text"),
                source: wgpu::ShaderSource::Wgsl(TEXT_WGSL.into()),
            });
        let layout = self
            .device()
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("xgc-text-layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(16),
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
                    label: Some("xgc-text-pipeline-layout"),
                    bind_group_layouts: &[&layout],
                    push_constant_ranges: &[],
                });
        let pipeline = self
            .device()
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("xgc-text-pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: TEXT_VERTEX_BYTES as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![
                            0 => Float32x2,
                            1 => Float32x2,
                            2 => Float32x4
                        ],
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
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview: None,
                cache: None,
            });
        let sampler = self.device().create_sampler(&wgpu::SamplerDescriptor {
            label: Some("xgc-text-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        TextPipeline {
            pipeline,
            layout,
            sampler,
        }
    }
}

/// 一次文本绘制的参数束（字形列表 + 图集 + 裁剪）。
pub struct TextDraw<'a> {
    pub target: &'a wgpu::Texture,
    pub atlas: &'a wgpu::Texture,
    pub viewport: [f32; 2],
    pub quads: &'a [GlyphQuad],
    pub color: [f32; 4],
    pub scissor: [u32; 4],
}

impl TextPipeline {
    /// 画一批字形；`quads` 是屏幕像素坐标，`color` 是该段文本的颜色。
    pub fn draw(&self, context: &GpuContext, request: &TextDraw<'_>) -> Result<(), GpuError> {
        let TextDraw {
            target,
            atlas,
            viewport,
            quads,
            color,
            scissor,
        } = *request;
        if quads.is_empty() || color[3] <= 0.0 {
            return Ok(());
        }
        let mut vertices: Vec<f32> = Vec::with_capacity(quads.len() * 6 * 8);
        for quad in quads {
            let [x0, y0] = [quad.x, quad.y];
            let [x1, y1] = [quad.x + quad.width, quad.y + quad.height];
            let [u0, v0, u1, v1] = quad.uv;
            let corners = [
                ([x0, y0], [u0, v0]),
                ([x1, y0], [u1, v0]),
                ([x1, y1], [u1, v1]),
                ([x0, y0], [u0, v0]),
                ([x1, y1], [u1, v1]),
                ([x0, y1], [u0, v1]),
            ];
            for (position, uv) in corners {
                vertices.extend_from_slice(&[position[0], position[1]]);
                vertices.extend_from_slice(&uv);
                vertices.extend_from_slice(&color);
            }
        }
        let mut bytes = Vec::with_capacity(vertices.len() * 4);
        for value in &vertices {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let vertex_buffer = context.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("xgc-text-vertices"),
            size: bytes.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        context.queue().write_buffer(&vertex_buffer, 0, &bytes);
        let uniform = context.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("xgc-text-screen"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bytes: Vec<u8> = [viewport[0], viewport[1], 0.0, 0.0]
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        context.queue().write_buffer(&uniform, 0, &uniform_bytes);
        let bind_group = context
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("xgc-text-bind"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(
                            &atlas.create_view(&wgpu::TextureViewDescriptor::default()),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            context
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("xgc-text-encoder"),
                });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xgc-text-pass"),
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
            pass.set_scissor_rect(scissor[0], scissor[1], scissor[2], scissor[3]);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            pass.draw(0..vertices.len() as u32 / 8, 0..1);
        }
        context.queue().submit(Some(encoder.finish()));
        Ok(())
    }
}
