//! FXAA（显示空间抗锯齿）：tone-map 之后、UI 之前的一趟全屏平滑。
//!
//! 与旧端 `xgame-render/src/postfx.rs` 同一条管线位置：
//! `HDR 场景 ──tone-map──▶ LDR scratch ──FXAA──▶ 显示目标`。UI 由调用方在
//! `present_scene` 之后直接画到显示目标，所以 HUD 文字不会被抗锯齿柔化。
//!
//! **默认关**，与光晕 / 天空同一条纪律：画质增强默认不动画面。关着时整段跳过
//! （不建 LDR scratch、不记 pass），帧输出与引入 FXAA 之前逐字节相同。
//!
//! 只在"边缘"上动手：五抽头范围低于阈值的像素原样返回中心采样，平坦区域因此
//! 不会整幅变软。阈值、亚像素混合与沿边搜索的口径见 `fxaa.wgsl` 顶部注释。

use std::sync::Arc;

use crate::gpu::GpuContext;

const FXAA_WGSL: &str = include_str!("fxaa.wgsl");
/// `[texel_x, texel_y, srgb, _pad]`，与 `fxaa.wgsl` 的 `FxaaParams` 逐字段对齐。
const PARAMS_BYTES: usize = 16;

struct Scratch {
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    texture: Arc<wgpu::Texture>,
}

struct Pipelines {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    params: wgpu::Buffer,
}

pub(crate) struct FxaaStage {
    pipelines: Pipelines,
    /// 上次写进 `params` 的四元组（按位比较，避免每帧都写缓冲）。
    params_written: Option<[u32; PARAMS_BYTES / 4]>,
    pipeline: Option<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    scratch: Option<Scratch>,
}

impl FxaaStage {
    pub(crate) fn new(context: &GpuContext) -> Self {
        let layout = context
            .device()
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("xgc-fxaa-layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            // 边缘搜索要按非整数 uv 采样，必须是可过滤的浮点纹理。
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });
        let sampler = context.device().create_sampler(&wgpu::SamplerDescriptor {
            label: Some("xgc-fxaa-sampler"),
            // 沿边搜索在 texel 之间有小数偏移，必须线性过滤；边界夹住，
            // 屏幕边缘的邻居退化成自己（旧端同款 ClampToEdge）。
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let params = context.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("xgc-fxaa-params"),
            size: PARAMS_BYTES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipelines: Pipelines {
                layout,
                sampler,
                params,
            },
            params_written: None,
            pipeline: None,
            scratch: None,
        }
    }

    /// 丢弃 LDR scratch：关闭态不养显存（旧端 `fxaa_on == false` 时同款）。
    pub(crate) fn release(&mut self) {
        self.scratch = None;
    }

    /// 返回 tone-map 的落点（显示空间 LDR 纹理）：尺寸与格式不变时跨帧复用。
    ///
    /// 格式必须与最终显示目标**完全一致**：FXAA 读写的都是显示目标格式，
    /// sRGB 与否决定着色器里那一次 encode/decode 往返走不走。
    pub(crate) fn scratch(
        &mut self,
        context: &GpuContext,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Arc<wgpu::Texture> {
        let width = width.max(1);
        let height = height.max(1);
        if self.scratch.as_ref().is_none_or(|scratch| {
            scratch.width != width || scratch.height != height || scratch.format != format
        }) {
            let texture = Arc::new(context.device().create_texture(&wgpu::TextureDescriptor {
                label: Some("xgc-fxaa-scratch"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            }));
            self.scratch = Some(Scratch {
                width,
                height,
                format,
                texture,
            });
        }
        let scratch = self.scratch.as_ref().expect("刚建过 scratch");
        // `wgpu::TextureView` 不是 `Clone`：交给调用方每次自己建视图（视图创建很便宜，
        // 且 tone-map 与 FXAA 用的是同一张纹理的两个不同用途）。
        Arc::clone(&scratch.texture)
    }

    /// 把 `source`（tone-map 的结果）平滑到 `target`（显示目标）；返回 1 趟 pass。
    pub(crate) fn present(
        &mut self,
        context: &GpuContext,
        source: &wgpu::Texture,
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
    ) -> usize {
        if self
            .pipeline
            .as_ref()
            .is_none_or(|(current, _)| *current != format)
        {
            self.pipeline = Some((format, self.create_pipeline(context, format)));
        }
        let pipeline = &self.pipeline.as_ref().expect("刚建过管线").1;
        let width = source.width().max(1) as f32;
        let height = source.height().max(1) as f32;
        let texel = [1.0 / width, 1.0 / height];
        let srgb = if format.is_srgb() { 1u32 } else { 0u32 };
        let bits = [texel[0].to_bits(), texel[1].to_bits(), srgb, 0];
        if self.params_written != Some(bits) {
            let mut bytes = [0u8; PARAMS_BYTES];
            bytes[0..4].copy_from_slice(&texel[0].to_le_bytes());
            bytes[4..8].copy_from_slice(&texel[1].to_le_bytes());
            bytes[8..12].copy_from_slice(&srgb.to_le_bytes());
            context
                .queue()
                .write_buffer(&self.pipelines.params, 0, &bytes);
            self.params_written = Some(bits);
        }
        let source_view = source.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = context
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("xgc-fxaa-bind"),
                layout: &self.pipelines.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&source_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.pipelines.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.pipelines.params.as_entire_binding(),
                    },
                ],
            });
        let mut encoder =
            context
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("xgc-fxaa-encoder"),
                });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xgc-fxaa-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // FXAA 每个像素都写，load 内容不会被读；Clear 是为了让
                        // 驱动不必先保存上一位图（旧端同款）。
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        context.queue().submit(Some(encoder.finish()));
        1
    }

    fn create_pipeline(
        &self,
        context: &GpuContext,
        format: wgpu::TextureFormat,
    ) -> wgpu::RenderPipeline {
        let shader = context
            .device()
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("xgc-fxaa"),
                source: wgpu::ShaderSource::Wgsl(FXAA_WGSL.into()),
            });
        let layout = context
            .device()
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("xgc-fxaa-pipeline-layout"),
                bind_group_layouts: &[&self.pipelines.layout],
                push_constant_ranges: &[],
            });
        context
            .device()
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("xgc-fxaa-pipeline"),
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
