//! HDR 场景中间目标与最终全屏 tone-map。
//!
//! 几何、天空与粒子先在线性 `Rgba16Float` 中合成；这里只在场景结束时执行一次
//! ACES。UI 由调用方随后直接画到显示目标，因此不会被场景曝光或曲线改变。

use std::sync::Arc;

use crate::gpu::GpuContext;

const TONE_MAP_WGSL: &str = include_str!("../tone_map.wgsl");
const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

struct HdrTarget {
    width: u32,
    height: u32,
    texture: Arc<wgpu::Texture>,
    bind: wgpu::BindGroup,
}

pub(super) struct HdrStage {
    texture_layout: wgpu::BindGroupLayout,
    target: Option<HdrTarget>,
    pipeline: Option<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
}

impl HdrStage {
    pub(super) fn new(context: &GpuContext) -> Self {
        let texture_layout =
            context
                .device()
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("xgc-hdr-texture-layout"),
                    entries: &[wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    }],
                });
        Self {
            texture_layout,
            target: None,
            pipeline: None,
        }
    }

    /// 返回与显示目标同尺寸的线性 HDR 纹理；尺寸不变时跨帧复用。
    pub(super) fn scene_texture(
        &mut self,
        context: &GpuContext,
        width: u32,
        height: u32,
    ) -> Arc<wgpu::Texture> {
        let width = width.max(1);
        let height = height.max(1);
        if self
            .target
            .as_ref()
            .is_none_or(|target| target.width != width || target.height != height)
        {
            let texture = Arc::new(context.device().create_texture(&wgpu::TextureDescriptor {
                label: Some("xgc-hdr-scene"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: HDR_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            }));
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind = context
                .device()
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("xgc-hdr-texture-bind"),
                    layout: &self.texture_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    }],
                });
            self.target = Some(HdrTarget {
                width,
                height,
                texture,
                bind,
            });
        }
        Arc::clone(&self.target.as_ref().unwrap().texture)
    }

    /// 把线性 HDR 场景一次性映射到显示目标；目标的 sRGB 格式负责最终传输编码。
    pub(super) fn present(&mut self, context: &GpuContext, output: &wgpu::Texture) {
        let format = output.format();
        if self
            .pipeline
            .as_ref()
            .is_none_or(|(current, _)| *current != format)
        {
            self.pipeline = Some((format, self.create_pipeline(context, format)));
        }
        let target = self.target.as_ref().expect("present 前必须创建 HDR 目标");
        let pipeline = &self.pipeline.as_ref().unwrap().1;
        let view = output.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            context
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("xgc-tone-map-encoder"),
                });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xgc-tone-map-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &target.bind, &[]);
            pass.draw(0..3, 0..1);
        }
        context.queue().submit(Some(encoder.finish()));
    }

    fn create_pipeline(
        &self,
        context: &GpuContext,
        format: wgpu::TextureFormat,
    ) -> wgpu::RenderPipeline {
        let shader = context
            .device()
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("xgc-tone-map"),
                source: wgpu::ShaderSource::Wgsl(TONE_MAP_WGSL.into()),
            });
        let layout = context
            .device()
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("xgc-tone-map-layout"),
                bind_group_layouts: &[&self.texture_layout],
                push_constant_ranges: &[],
            });
        context
            .device()
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("xgc-tone-map-pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[],
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
            })
    }
}

impl super::Renderer {
    pub(super) fn hdr_scene_texture(
        &mut self,
        context: &GpuContext,
        width: u32,
        height: u32,
    ) -> Arc<wgpu::Texture> {
        self.hdr
            .get_or_insert_with(|| HdrStage::new(context))
            .scene_texture(context, width, height)
    }

    pub(super) fn present_hdr(&mut self, context: &GpuContext, output: &wgpu::Texture) {
        self.hdr
            .as_mut()
            .expect("HDR 目标必须先完成场景渲染")
            .present(context, output);
    }
}
