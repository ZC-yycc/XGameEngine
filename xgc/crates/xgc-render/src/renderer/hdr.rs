//! HDR 场景中间目标、光晕合成与最终全屏 tone-map。
//!
//! 几何、天空与粒子先在线性 `Rgba16Float` 中合成；场景结束时按需跑一遍光晕链，
//! 再由唯一一趟 ACES pass 把「场景 + 光晕」写进显示目标。UI 由调用方随后直接
//! 画到显示目标，因此不会被场景曝光或曲线改变。

use std::sync::Arc;

use crate::bloom::{BloomSettings, BLOOM_LEVELS, BLOOM_WEIGHTS};
use crate::gpu::GpuContext;

const TONE_MAP_WGSL: &str = include_str!("../tone_map.wgsl");
const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// `[intensity, w0, w1, w2]`，与 `tone_map.wgsl` 的 `BloomParams` 逐字段对齐。
const BLOOM_PARAMS_BYTES: usize = 16;

struct HdrTarget {
    width: u32,
    height: u32,
    texture: Arc<wgpu::Texture>,
    /// 场景视图：既是 tone-map 的输入，也是光晕关闭时三级光晕的占位绑定。
    view: wgpu::TextureView,
}

pub(super) struct HdrStage {
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    bloom_params: wgpu::Buffer,
    /// 上次写进 `bloom_params` 的四元组（按位比较，避免每帧都写缓冲）。
    bloom_params_written: Option<[u32; BLOOM_PARAMS_BYTES / 4]>,
    target: Option<HdrTarget>,
    /// 光晕关闭时的绑定组：三级输入全指向场景纹理占位、强度恒 0，
    /// 只有场景纹理重建才需要换，跨帧缓存即可（旧实现就是缓存的）。
    closed_bind: Option<wgpu::BindGroup>,
    pipeline: Option<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
}

impl HdrStage {
    pub(super) fn new(context: &GpuContext) -> Self {
        let texture_layout =
            context
                .device()
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("xgc-hdr-texture-layout"),
                    entries: &[
                        // 场景按像素 textureLoad，逐位取原值。
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        // 光晕三级纹理比场景小，合成为把半分辨率放大回去要走采样器。
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
                        wgpu::BindGroupLayoutEntry {
                            binding: 3,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 4,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 5,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                    ],
                });
        let sampler = context.device().create_sampler(&wgpu::SamplerDescriptor {
            label: Some("xgc-tone-map-glow-sampler"),
            // 三级光晕尺寸各差一倍，放大回全分辨率必须双线性；边界夹住即可。
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let bloom_params = context.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("xgc-tone-map-bloom-params"),
            size: BLOOM_PARAMS_BYTES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            texture_layout,
            sampler,
            bloom_params,
            bloom_params_written: None,
            target: None,
            closed_bind: None,
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
            self.target = Some(HdrTarget {
                width,
                height,
                texture,
                view,
            });
            // 占位绑定指向旧纹理的视图，随目标一起作废。
            self.closed_bind = None;
        }
        Arc::clone(&self.target.as_ref().unwrap().texture)
    }

    /// 把线性 HDR 场景（可选叠加光晕）一次性映射到显示目标；目标的 sRGB 格式负责
    /// 最终传输编码。
    pub(super) fn present(
        &mut self,
        context: &GpuContext,
        output: &wgpu::Texture,
        bloom: BloomSettings,
        glow: Option<[&wgpu::TextureView; BLOOM_LEVELS]>,
    ) {
        let format = output.format();
        if self
            .pipeline
            .as_ref()
            .is_none_or(|(current, _)| *current != format)
        {
            self.pipeline = Some((format, self.create_pipeline(context, format)));
        }
        let pipeline = &self.pipeline.as_ref().unwrap().1;
        let weights = [
            bloom.intensity,
            BLOOM_WEIGHTS[0],
            BLOOM_WEIGHTS[1],
            BLOOM_WEIGHTS[2],
        ];
        let bits = weights.map(f32::to_bits);
        if self.bloom_params_written != Some(bits) {
            let mut bytes = [0u8; BLOOM_PARAMS_BYTES];
            for (slot, value) in weights.iter().enumerate() {
                bytes[slot * 4..slot * 4 + 4].copy_from_slice(&value.to_le_bytes());
            }
            context.queue().write_buffer(&self.bloom_params, 0, &bytes);
            self.bloom_params_written = Some(bits);
        }
        // 关闭态：三级输入全指向场景纹理占位、强度恒 0，绑定组跨帧缓存（场景纹理
        // 重建时清掉）。开启态每帧重建 —— 场景纹理与光晕链各有独立的重建判据，
        // 跨帧缓存要维护多重失效条件，收益抵不上一次绑定创建。
        let fresh: Option<wgpu::BindGroup>;
        let bind: &wgpu::BindGroup = match glow {
            Some(glow) => {
                let target = self.target.as_ref().expect("present 前必须创建 HDR 目标");
                fresh = Some(create_bind(
                    context,
                    &self.texture_layout,
                    &self.sampler,
                    &self.bloom_params,
                    &target.view,
                    glow,
                ));
                fresh.as_ref().expect("刚建过绑定组")
            }
            None => {
                if self.closed_bind.is_none() {
                    let cached = {
                        let target = self.target.as_ref().expect("present 前必须创建 HDR 目标");
                        let placeholder = [&target.view; BLOOM_LEVELS];
                        create_bind(
                            context,
                            &self.texture_layout,
                            &self.sampler,
                            &self.bloom_params,
                            &target.view,
                            placeholder,
                        )
                    };
                    self.closed_bind = Some(cached);
                }
                self.closed_bind.as_ref().expect("刚建过绑定组")
            }
        };
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
            pass.set_bind_group(0, bind, &[]);
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

/// tone-map 的 6 项绑定：场景（`textureLoad`）、光晕采样器、参数、三级光晕。
fn create_bind(
    context: &GpuContext,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    params: &wgpu::Buffer,
    scene: &wgpu::TextureView,
    glow: [&wgpu::TextureView; BLOOM_LEVELS],
) -> wgpu::BindGroup {
    context
        .device()
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("xgc-hdr-texture-bind"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(scene),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(glow[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(glow[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(glow[2]),
                },
            ],
        })
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

    /// 场景收尾：按需跑光晕链，再把「场景 + 光晕」tone-map 到显示目标。
    pub(super) fn present_scene(
        &mut self,
        context: &GpuContext,
        output: &wgpu::Texture,
        scene: &Arc<wgpu::Texture>,
        stats: &mut super::RenderStats,
    ) {
        let bloom = self.bloom;
        if bloom.is_enabled() {
            let stage = self
                .bloom_stage
                .get_or_insert_with(|| crate::bloom::BloomStage::new(context));
            stats.passes += stage.render(context, scene, bloom);
        } else if let Some(stage) = self.bloom_stage.as_mut() {
            // 旧端在 `intensity == 0` 时把链整份丢掉：留着就白占显存，
            // 关闭态还会把上一轮的残留视图绑进 tone-map。
            stage.release();
        }
        // 关着时链已被释放（`views()` 回 `None`），这里退化成绑场景纹理占位：
        // 强度写 0，着色器整段分支跳过，绑什么都不会被采样。
        let glow = self.bloom_stage.as_ref().and_then(|stage| stage.views());
        self.hdr
            .as_mut()
            .expect("HDR 目标必须先完成场景渲染")
            .present(context, output, bloom, glow);
        stats.passes += 1;
    }
}

#[cfg(test)]
impl super::Renderer {
    /// 只给测试：关掉光晕后链必须整份释放（旧端 `intensity == 0` 丢链的合同）。
    pub(crate) fn bloom_chain_size(&self) -> Option<(u32, u32)> {
        self.bloom_stage
            .as_ref()
            .and_then(|stage| stage.chain_size())
    }
}

#[cfg(test)]
mod tests {
    use xgc_world::World;

    use crate::assets::MeshCache;
    use crate::bloom::BloomSettings;
    use crate::gpu::{GpuContext, GpuResource};
    use crate::resources::{ResourceDesc, ResourceUsage};
    use crate::Renderer;

    /// 集成用例只能看像素：关掉光晕后画面照样对，但链留着白占显存。
    /// 显存侧没有外部可观测口径，所以在 crate 内直接看链还在不在。
    #[test]
    fn turning_bloom_off_releases_the_chain() {
        let Ok(context) = GpuContext::headless() else {
            eprintln!("skip: 没有可用 GPU 适配器");
            return;
        };
        let desc = ResourceDesc::texture2d(
            "hdr-chain-target",
            "rgba8unorm",
            8,
            8,
            vec![ResourceUsage::ColorAttachment],
        );
        let GpuResource::Texture(texture) = context.create_resource(&desc).unwrap() else {
            panic!("渲染目标不是纹理");
        };
        let world = World::new();
        let mut renderer = Renderer::new(MeshCache::new("."));
        renderer.set_clear_color([1.4, 1.4, 1.4, 1.0]);
        renderer.set_bloom(BloomSettings {
            intensity: 1.0,
            ..BloomSettings::default()
        });

        let stats = renderer.render(&context, &texture, &world).unwrap();
        assert!(
            stats.passes == 11,
            "开着光晕应是场景 + 9 段链 + tone-map：{}",
            stats.passes
        );
        assert!(
            renderer.bloom_chain_size() == Some((8, 8)),
            "开着时链必须跟着场景尺寸存在：{:?}",
            renderer.bloom_chain_size()
        );

        renderer.set_bloom(BloomSettings {
            intensity: 0.0,
            ..BloomSettings::default()
        });
        let stats = renderer.render(&context, &texture, &world).unwrap();
        assert!(
            stats.passes == 2,
            "关掉后只该有场景 + tone-map：{}",
            stats.passes
        );
        assert!(
            renderer.bloom_chain_size().is_none(),
            "关掉后链必须整份释放：{:?}",
            renderer.bloom_chain_size()
        );
    }
}
